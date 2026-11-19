#include "xelab/sdk.h"

#include <algorithm>
#include <utility>

#include <grpcpp/generic/generic_stub.h>

namespace xelab::sdk {
namespace {

constexpr char kExecute[] = "/xelab.v1.TaskWorker/Execute";
constexpr uint64_t kMaxSafeCounter = (uint64_t{1} << 53) - 1;
constexpr size_t kMaxInputBytes = 4 * 1024 * 1024;
constexpr size_t kMaxOutputBytes = 16 * 1024 * 1024;
constexpr auto kExecuteTimeLimit = std::chrono::hours(24);

Deadline ExecuteDeadline(const v1::ExecuteTaskRequest& request, Deadline deadline) {
  using std::chrono::milliseconds;
  const auto limit = std::min(deadline, std::chrono::system_clock::now() + kExecuteTimeLimit);
  const auto request_ms = request.context().deadline_unix_ms();
  const auto limit_ms = std::chrono::duration_cast<milliseconds>(limit.time_since_epoch());
  if (request_ms > 0 && milliseconds(request_ms) <= limit_ms) {
    return std::min(limit, Deadline(milliseconds(request_ms)));
  }
  return limit;
}

char AsciiLower(char ch) {
  return ch >= 'A' && ch <= 'F' ? ch + ('a' - 'A') : ch;
}

std::string NormalizedUuid(std::string_view value) {
  if (value.size() != 36) return {};
  std::string normalized(value);
  std::transform(normalized.begin(), normalized.end(), normalized.begin(), AsciiLower);
  return IsUuid(normalized) ? normalized : std::string{};
}

bool ValidUtf8(std::string_view value) {
  while (!value.empty()) {
    const auto first = static_cast<unsigned char>(value.front());
    value.remove_prefix(1);
    if (first <= 0x7f) continue;
    if (first < 0xc2 || first > 0xf4) return false;
    const size_t length = first < 0xe0 ? 1 : first < 0xf0 ? 2 : 3;
    if (value.size() < length) return false;

    const auto second = static_cast<unsigned char>(value.front());
    // Exclude overlong encodings, surrogate code points and values above U+10FFFF.
    if ((first == 0xe0 && second < 0xa0) || (first == 0xed && second >= 0xa0) ||
        (first == 0xf0 && second < 0x90) || (first == 0xf4 && second >= 0x90)) {
      return false;
    }
    for (const unsigned char byte : value.substr(0, length)) {
      if (byte < 0x80 || byte > 0xbf) return false;
    }
    value.remove_prefix(length);
  }
  return true;
}

std::optional<TaskOutcome> ReadOutcome(v1::TaskResult& result) {
  switch (result.state()) {
    case v1::EXECUTION_STATE_SUCCEEDED:
      if (result.has_output_json() && !result.output_json().empty() &&
          result.output_json().size() <= kMaxOutputBytes && ValidUtf8(result.output_json())) {
        return TaskSucceeded{std::move(*result.mutable_output_json())};
      }
      break;
    case v1::EXECUTION_STATE_FAILED:
      if (result.has_error() && !result.error().code().empty() && !result.error().message().empty()) {
        return TaskFailed{std::move(*result.mutable_error())};
      }
      break;
    case v1::EXECUTION_STATE_CANCELED:
      if (result.outcome_case() == v1::TaskResult::OUTCOME_NOT_SET) return TaskCanceled{};
      break;
    case v1::EXECUTION_STATE_UNKNOWN:
      if (result.outcome_case() == v1::TaskResult::OUTCOME_NOT_SET) return TaskUnknown{};
      break;
    default:
      break;
  }
  return std::nullopt;
}

}  // namespace

// Receive bytes before parsing so a protobuf decoding failure cannot be
// mistaken for end of stream by the generated C++ ClientReader.
class TaskProgressReader::Stream {
 public:
  explicit Stream(Deadline deadline) { context_.set_deadline(deadline); }

  ~Stream() {
    if (call_) {
      context_.TryCancel();
      Finish();
    }
    queue_.Shutdown();
    void* tag;
    bool ok;
    while (queue_.Next(&tag, &ok)) {
    }
  }

  grpc::Status Open(std::shared_ptr<grpc::ChannelInterface> channel, std::string_view request) {
    call_ = grpc::GenericStub(std::move(channel)).PrepareCall(&context_, kExecute, &queue_);
    if (!call_) return {grpc::StatusCode::UNKNOWN, "Could not prepare Execute"};
    call_->StartCall(this);
    if (!Await()) return FailedOpen();
    const grpc::Slice slice(request.data(), request.size());
    call_->WriteLast(grpc::ByteBuffer(&slice, 1), grpc::WriteOptions{}, this);
    return Await() ? grpc::Status::OK : FailedOpen();
  }

  bool Read(grpc::ByteBuffer& bytes) {
    call_->Read(&bytes, this);
    return Await();
  }

  grpc::Status Finish() {
    grpc::Status status;
    call_->Finish(&status, this);
    const bool received = Await();
    call_.reset();
    return received ? status : grpc::Status(grpc::StatusCode::UNKNOWN, "Missing Execute status");
  }

 private:
  bool Await() {
    void* tag;
    bool ok;
    return queue_.Next(&tag, &ok) && tag == this && ok;
  }

  grpc::Status FailedOpen() {
    const auto status = Finish();
    return status.ok() ? grpc::Status(grpc::StatusCode::UNKNOWN, "Could not start Execute") : status;
  }

  grpc::ClientContext context_;
  grpc::CompletionQueue queue_;
  std::unique_ptr<grpc::GenericClientAsyncReaderWriter> call_;
};

TaskProgressReader::TaskProgressReader(std::shared_ptr<grpc::ChannelInterface> channel,
                                       const v1::ExecuteTaskRequest& request, Deadline deadline)
    : deadline_(ExecuteDeadline(request, deadline)),
      execution_id_(NormalizedUuid(request.execution_id())), fence_(request.fence()) {
  if (!channel || execution_id_.empty() || fence_ == 0 || fence_ > kMaxSafeCounter ||
      request.input_json().size() > kMaxInputBytes ||
      request.ByteSizeLong() > kMaxMessageBytes) {
    Fail(ProgressError::InvalidCall);
    return;
  }
  std::string bytes;
  if (!request.SerializeToString(&bytes)) {
    Fail(ProgressError::InvalidCall);
    return;
  }
  if (std::chrono::system_clock::now() >= deadline_) {
    Fail(ProgressError::DeadlineExceeded);
    return;
  }
  stream_ = std::make_unique<Stream>(deadline_);
  const auto status = stream_->Open(std::move(channel), bytes);
  if (status.error_code() == grpc::StatusCode::DEADLINE_EXCEEDED ||
      std::chrono::system_clock::now() >= deadline_) {
    Fail(ProgressError::DeadlineExceeded);
  } else if (!status.ok()) {
    Fail(ProgressError::Transport, status.error_code());
  }
}

TaskProgressReader::~TaskProgressReader() = default;

ProgressFailure TaskProgressReader::Fail(ProgressError reason, grpc::StatusCode code) {
  stream_.reset();
  return failure_.emplace(ProgressFailure{reason, code});
}

bool TaskProgressReader::Matches(std::string_view execution_id, uint64_t fence) const {
  return fence == fence_ && execution_id.size() == execution_id_.size() &&
         std::equal(execution_id.begin(), execution_id.end(), execution_id_.begin(),
                    [](char received, char expected) { return AsciiLower(received) == expected; });
}

ProgressRead TaskProgressReader::Next() {
  if (failure_) return *failure_;
  if (!stream_) return EndOfProgress{};
  if (std::chrono::system_clock::now() >= deadline_) return Fail(ProgressError::DeadlineExceeded);

  std::optional<TaskOutcome> outcome;
  grpc::ByteBuffer bytes;
  if (stream_->Read(bytes)) {
    grpc::Slice slice;
    v1::TaskProgress frame;
    if (bytes.Length() > kMaxMessageBytes || !bytes.DumpToSingleSlice(&slice).ok() ||
        !frame.ParseFromArray(slice.begin(), static_cast<int>(slice.size()))) {
      return Fail(ProgressError::MalformedFrame);
    }
    if (std::chrono::system_clock::now() >= deadline_) return Fail(ProgressError::DeadlineExceeded);
    if (!Matches(frame.execution_id(), frame.fence())) return Fail(ProgressError::WrongIdentity);
    if (frame.sequence() != next_sequence_ || frame.sequence() > kMaxSafeCounter) {
      return Fail(ProgressError::WrongSequence);
    }
    ++next_sequence_;
    switch (frame.body_case()) {
      case v1::TaskProgress::kHeartbeat:
        if (frame.heartbeat().progress_percent() > 100) return Fail(ProgressError::InvalidHeartbeat);
        return std::move(*frame.mutable_heartbeat());
      case v1::TaskProgress::kCheckpoint:
        if (NormalizedUuid(frame.checkpoint().blob_id()).empty() ||
            frame.checkpoint().data_schema_version().empty() ||
            frame.checkpoint().plugin_version().empty()) {
          return Fail(ProgressError::InvalidCheckpoint);
        }
        return std::move(*frame.mutable_checkpoint());
      case v1::TaskProgress::kResult:
        if (!Matches(frame.result().execution_id(), frame.result().fence())) {
          return Fail(ProgressError::WrongIdentity);
        }
        outcome = ReadOutcome(*frame.mutable_result());
        if (!outcome) return Fail(ProgressError::InvalidTerminal);
        if (std::chrono::system_clock::now() >= deadline_) return Fail(ProgressError::DeadlineExceeded);
        bytes.Clear();
        if (stream_->Read(bytes)) return Fail(ProgressError::TrailingFrame);
        break;
      case v1::TaskProgress::BODY_NOT_SET:
        return Fail(ProgressError::MissingBody);
    }
  }

  const auto status = stream_->Finish();
  if (status.error_code() == grpc::StatusCode::DEADLINE_EXCEEDED ||
      std::chrono::system_clock::now() >= deadline_) {
    return Fail(ProgressError::DeadlineExceeded);
  }
  if (!status.ok()) return Fail(ProgressError::Transport, status.error_code());
  if (!outcome) return Fail(ProgressError::MissingTerminal);
  stream_.reset();
  return std::move(*outcome);
}

}  // namespace xelab::sdk
