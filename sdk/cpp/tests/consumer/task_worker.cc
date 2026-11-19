#include <xelab/sdk.h>

#include <array>
#include <atomic>
#include <future>
#include <iostream>
#include <stdexcept>
#include <thread>
#include <utility>
#include <vector>

#include <arpa/inet.h>
#include <grpc/grpc.h>
#include <sys/socket.h>
#include <unistd.h>

namespace {

namespace sdk = xelab::sdk;
namespace v1 = xelab::v1;
using Clock = std::chrono::system_clock;
using namespace std::chrono_literals;

constexpr char kExecution[] = "a0000000-0000-4000-8000-000000000001";
constexpr char kCheckpoint[] = "00000000-0000-4000-8000-000000000002";
constexpr uint64_t kFence = 7;

void Require(bool expected, const char* context) {
  if (!expected) throw std::runtime_error(context);
}

v1::ExecuteTaskRequest Request() {
  v1::ExecuteTaskRequest request;
  request.set_execution_id(kExecution);
  request.set_fence(kFence);
  return request;
}

v1::TaskProgress Frame(uint64_t sequence) {
  v1::TaskProgress frame;
  frame.set_execution_id(kExecution);
  frame.set_fence(kFence);
  frame.set_sequence(sequence);
  return frame;
}

v1::TaskProgress Heartbeat(uint64_t sequence = 1) {
  auto frame = Frame(sequence);
  frame.mutable_heartbeat()->set_progress_percent(17);
  frame.mutable_heartbeat()->set_phase("work");
  return frame;
}

v1::TaskProgress Terminal(v1::ExecutionState state, uint64_t sequence = 1) {
  auto frame = Frame(sequence);
  auto* result = frame.mutable_result();
  result->set_execution_id(kExecution);
  result->set_fence(kFence);
  result->set_state(state);
  return frame;
}

class Script final : public v1::TaskWorker::Service {
 public:
  explicit Script(std::vector<v1::TaskProgress> frames,
                  grpc::Status final_status = grpc::Status::OK, bool wait_for_cancel = false)
      : frames_(std::move(frames)), final_status_(std::move(final_status)),
        wait_for_cancel_(wait_for_cancel) {}

  grpc::Status Execute(grpc::ServerContext* context, const v1::ExecuteTaskRequest* request,
                       grpc::ServerWriter<v1::TaskProgress>* writer) override {
    ++calls;
    if (request->execution_id() != kExecution || request->fence() != kFence) {
      return {grpc::StatusCode::INVALID_ARGUMENT, "Unexpected fixture identity"};
    }
    for (const auto& frame : frames_) {
      if (!writer->Write(frame)) return {grpc::StatusCode::CANCELLED, "Client closed"};
    }
    if (wait_for_cancel_) {
      while (!context->IsCancelled()) std::this_thread::sleep_for(2ms);
    }
    return final_status_;
  }

  std::atomic<int> calls{0};

 private:
  const std::vector<v1::TaskProgress> frames_;
  const grpc::Status final_status_;
  const bool wait_for_cancel_;
};

class Loopback {
 public:
  explicit Loopback(grpc::Service& worker) {
    grpc::ServerBuilder builder;
    builder.RegisterService(&worker);
    int port = 0;
    builder.AddListeningPort("127.0.0.1:0", grpc::InsecureServerCredentials(), &port);
    server_ = builder.BuildAndStart();
    Require(server_ && port != 0, "could not start loopback TaskWorker");
    channel = Channel(port);
    Require(channel->WaitForConnected(Clock::now() + 3s), "loopback TaskWorker did not connect");
  }

  static std::shared_ptr<grpc::Channel> Channel(int port) {
    grpc::ChannelArguments arguments;
    arguments.SetMaxReceiveMessageSize(sdk::kMaxMessageBytes);
    arguments.SetMaxSendMessageSize(sdk::kMaxMessageBytes);
    arguments.SetInt(GRPC_ARG_ENABLE_RETRIES, 0);
    return grpc::CreateCustomChannel("127.0.0.1:" + std::to_string(port),
                                     grpc::InsecureChannelCredentials(), arguments);
  }

  std::shared_ptr<grpc::Channel> channel;

 private:
  std::unique_ptr<grpc::Server> server_;
};

void CheckError(sdk::ProgressRead event, sdk::ProgressError reason, const char* label,
                grpc::StatusCode code = grpc::StatusCode::OK) {
  const auto* failure = std::get_if<sdk::ProgressFailure>(&event);
  Require(failure && failure->reason == reason && failure->transport_code == code, label);
}

void CheckScriptError(std::vector<v1::TaskProgress> frames, sdk::ProgressError reason,
                      const char* label, grpc::Status status = grpc::Status::OK) {
  Script worker(std::move(frames), status);
  Loopback loopback(worker);
  sdk::TaskProgressReader reader(loopback.channel, Request(), Clock::now() + 3s);
  CheckError(reader.Next(), reason, label, status.error_code());
  CheckError(reader.Next(), reason, "stream failure was not sticky", status.error_code());
  Require(worker.calls == 1, "stream started more or fewer than once");
}

void CheckEvents() {
  auto heartbeat = Heartbeat();
  heartbeat.set_execution_id("A0000000-0000-4000-8000-000000000001");
  auto checkpoint = Frame(2);
  checkpoint.mutable_checkpoint()->set_blob_id(kCheckpoint);
  checkpoint.mutable_checkpoint()->set_data_schema_version("1");
  checkpoint.mutable_checkpoint()->set_plugin_version("1.0.0");
  auto result = Terminal(v1::EXECUTION_STATE_SUCCEEDED, 3);
  const std::string body = "{\"example\":\"\xe4\xb8\xad\xf0\x9f\x98\x80\"}";
  result.mutable_result()->set_output_json(body);
  Script worker({heartbeat, checkpoint, result});
  Loopback loopback(worker);
  sdk::TaskProgressReader reader(loopback.channel, Request(), Clock::now() + 3s);
  Require(std::get<v1::Heartbeat>(reader.Next()).progress_percent() == 17, "missing heartbeat");
  Require(std::get<v1::Checkpoint>(reader.Next()).blob_id() == kCheckpoint, "missing checkpoint");
  const auto outcome = std::get<sdk::TaskOutcome>(reader.Next());
  Require(std::get<sdk::TaskSucceeded>(outcome).output_json == body, "wrong success output");
  Require(std::holds_alternative<sdk::EndOfProgress>(reader.Next()), "missing confirmed EOF");
  Require(worker.calls == 1, "stream started more or fewer than once");

  auto failed = Terminal(v1::EXECUTION_STATE_FAILED);
  auto* error = failed.mutable_result()->mutable_error();
  error->set_code("INVALID_ARGUMENT");
  error->set_message("Fixture failure");
  for (const auto& terminal : {failed, Terminal(v1::EXECUTION_STATE_CANCELED),
                              Terminal(v1::EXECUTION_STATE_UNKNOWN)}) {
    Script terminal_worker({terminal});
    Loopback terminal_loopback(terminal_worker);
    sdk::TaskProgressReader terminal_reader(terminal_loopback.channel, Request(), Clock::now() + 3s);
    const auto terminal_outcome = std::get<sdk::TaskOutcome>(terminal_reader.Next());
    switch (terminal.result().state()) {
      case v1::EXECUTION_STATE_FAILED:
        Require(std::get<sdk::TaskFailed>(terminal_outcome).error.code() == "INVALID_ARGUMENT",
                "wrong failure code");
        break;
      case v1::EXECUTION_STATE_CANCELED:
        Require(std::holds_alternative<sdk::TaskCanceled>(terminal_outcome), "missing canceled outcome");
        break;
      default:
        Require(std::holds_alternative<sdk::TaskUnknown>(terminal_outcome), "missing unknown outcome");
        break;
    }
  }
}

void CheckInvalidProgress() {
  auto bad_identity = Heartbeat();
  bad_identity.set_execution_id(kCheckpoint);
  CheckScriptError({bad_identity}, sdk::ProgressError::WrongIdentity, "wrong execution ID accepted");
  auto bad_fence = Heartbeat();
  bad_fence.set_fence(kFence + 1);
  CheckScriptError({bad_fence}, sdk::ProgressError::WrongIdentity, "wrong fence accepted");
  auto bad_inner = Terminal(v1::EXECUTION_STATE_UNKNOWN);
  bad_inner.mutable_result()->set_fence(kFence + 1);
  CheckScriptError({bad_inner}, sdk::ProgressError::WrongIdentity, "wrong terminal fence accepted");
  CheckScriptError({Heartbeat(2)}, sdk::ProgressError::WrongSequence, "noninitial sequence accepted");
  CheckScriptError({Heartbeat(uint64_t{1} << 53)}, sdk::ProgressError::WrongSequence,
                   "out of range sequence accepted");
  CheckScriptError({Frame(1)}, sdk::ProgressError::MissingBody, "missing body accepted");
  auto bad_heartbeat = Heartbeat();
  bad_heartbeat.mutable_heartbeat()->set_progress_percent(101);
  CheckScriptError({bad_heartbeat}, sdk::ProgressError::InvalidHeartbeat, "bad heartbeat accepted");
  auto bad_checkpoint = Frame(1);
  bad_checkpoint.mutable_checkpoint()->set_blob_id(kCheckpoint);
  CheckScriptError({bad_checkpoint}, sdk::ProgressError::InvalidCheckpoint, "unversioned checkpoint accepted");
  CheckScriptError({Terminal(v1::EXECUTION_STATE_RUNNING)}, sdk::ProgressError::InvalidTerminal,
                   "nonterminal state accepted");
  auto bad_result = Terminal(v1::EXECUTION_STATE_SUCCEEDED);
  CheckScriptError({bad_result}, sdk::ProgressError::InvalidTerminal, "missing output accepted");
  for (const char* invalid : {"\xff", "\xe0\x80\x80", "\xed\xa0\x80", "\xf4\x90\x80\x80", "\xc3"}) {
    bad_result.mutable_result()->set_output_json(invalid);
    CheckScriptError({bad_result}, sdk::ProgressError::InvalidTerminal, "invalid UTF-8 accepted");
  }
  bad_result.mutable_result()->set_output_json(std::string(16 * 1024 * 1024 + 1, 'a'));
  CheckScriptError({bad_result}, sdk::ProgressError::InvalidTerminal, "out of range output size accepted");
}

void CheckCompletion() {
  auto terminal = Terminal(v1::EXECUTION_STATE_UNKNOWN);
  CheckScriptError({}, sdk::ProgressError::MissingTerminal, "premature EOF accepted");
  CheckScriptError({terminal, Terminal(v1::EXECUTION_STATE_CANCELED, 2)},
                   sdk::ProgressError::TrailingFrame, "extra terminal accepted");
  CheckScriptError({terminal}, sdk::ProgressError::Transport, "non-OK final status accepted",
                   {grpc::StatusCode::UNAVAILABLE, "Fixture interrupted"});

  Script sequence_worker({Heartbeat(), Heartbeat()});
  Loopback sequence_loopback(sequence_worker);
  sdk::TaskProgressReader sequence_reader(sequence_loopback.channel, Request(), Clock::now() + 3s);
  Require(std::holds_alternative<v1::Heartbeat>(sequence_reader.Next()), "missing first heartbeat");
  CheckError(sequence_reader.Next(), sdk::ProgressError::WrongSequence, "repeated sequence accepted");

  auto delayed_terminal = Terminal(v1::EXECUTION_STATE_UNKNOWN, 2);
  Script stalled_worker({Heartbeat(), delayed_terminal}, grpc::Status::OK, true);
  Loopback stalled(stalled_worker);
  auto request = Request();
  const auto request_deadline = std::chrono::duration_cast<std::chrono::milliseconds>(
      (Clock::now() + 500ms).time_since_epoch());
  request.mutable_context()->set_deadline_unix_ms(request_deadline.count());
  sdk::TaskProgressReader stalled_reader(stalled.channel, request, Clock::now() + 3s);
  Require(std::holds_alternative<v1::Heartbeat>(stalled_reader.Next()), "no progress before timeout");
  CheckError(stalled_reader.Next(), sdk::ProgressError::DeadlineExceeded, "stalled terminal confirmed");

  Script unsent_worker({terminal});
  Loopback unsent(unsent_worker);
  auto invalid = Request();
  invalid.set_fence(0);
  sdk::TaskProgressReader invalid_reader(unsent.channel, invalid, Clock::now() + 3s);
  CheckError(invalid_reader.Next(), sdk::ProgressError::InvalidCall, "invalid fence was accepted");
  auto large = Request();
  large.set_input_json(std::string(4 * 1024 * 1024 + 1, 'a'));
  sdk::TaskProgressReader large_reader(unsent.channel, large, Clock::now() + 3s);
  CheckError(large_reader.Next(), sdk::ProgressError::InvalidCall, "oversize input was accepted");
  sdk::TaskProgressReader expired_reader(unsent.channel, Request(), Clock::now());
  CheckError(expired_reader.Next(), sdk::ProgressError::DeadlineExceeded, "expired call was accepted");
  Require(unsent_worker.calls == 0, "invalid or expired call reached the server");
}

class Socket {
 public:
  explicit Socket(int fd) : fd_(fd) { Require(fd_ >= 0, "could not open fixture socket"); }
  ~Socket() { close(fd_); }
  Socket(const Socket&) = delete;
  Socket& operator=(const Socket&) = delete;
  int fd() const { return fd_; }

 private:
  int fd_;
};

void ReadExact(int fd, void* buffer, size_t remaining) {
  auto* bytes = static_cast<char*>(buffer);
  while (remaining != 0) {
    const auto count = recv(fd, bytes, remaining, 0);
    Require(count > 0, "wire fixture input ended early");
    bytes += count;
    remaining -= count;
  }
}

void SendExact(int fd, std::string_view bytes) {
  while (!bytes.empty()) {
    const auto count = send(fd, bytes.data(), bytes.size(), MSG_NOSIGNAL);
    Require(count > 0, "wire fixture output ended early");
    bytes.remove_prefix(count);
  }
}

void SendFrame(int fd, uint8_t type, uint8_t flags, uint32_t stream, std::string_view payload) {
  const auto length = payload.size();
  const uint8_t header[] = {static_cast<uint8_t>(length >> 16), static_cast<uint8_t>(length >> 8),
                            static_cast<uint8_t>(length), type, flags,
                            static_cast<uint8_t>(stream >> 24), static_cast<uint8_t>(stream >> 16),
                            static_cast<uint8_t>(stream >> 8), static_cast<uint8_t>(stream)};
  SendExact(fd, {reinterpret_cast<const char*>(header), sizeof(header)});
  SendExact(fd, payload);
}

uint32_t ReceiveRequest(int fd) {
  std::array<char, 24> preface;
  ReadExact(fd, preface.data(), preface.size());
  Require(std::string_view(preface.data(), preface.size()) == "PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n",
          "wire fixture did not receive HTTP/2 preface");
  SendFrame(fd, 4, 0, 0, {});  // SETTINGS
  for (int frame = 0; frame < 16; ++frame) {
    std::array<uint8_t, 9> header;
    ReadExact(fd, header.data(), header.size());
    const size_t length = (header[0] << 16) | (header[1] << 8) | header[2];
    Require(length <= 16384, "oversize fixture request frame");
    std::array<char, 16384> payload;
    ReadExact(fd, payload.data(), length);
    const uint8_t type = header[3];
    const uint8_t flags = header[4];
    const uint32_t stream = (uint32_t{header[5] & 0x7fu} << 24) |
                            (uint32_t{header[6]} << 16) | (uint32_t{header[7]} << 8) | header[8];
    if (type == 4 && flags == 0) SendFrame(fd, 4, 1, 0, {});  // SETTINGS ACK
    if (stream != 0 && (flags & 1) != 0 && (type == 0 || type == 1)) return stream;
  }
  throw std::runtime_error("wire fixture did not receive a complete request");
}

std::string GrpcRecord(std::string_view message) {
  Require(message.size() < 256, "oversize fixture gRPC record");
  std::string record(5, '\0');  // Uncompressed flag and big-endian message size.
  record.back() = static_cast<char>(message.size());
  record.append(message);
  return record;
}

enum class WireEnding { Ok, BadProtobuf, ExtraBadProtobuf, MissingStatus, Disconnect };

// A generated gRPC service always encodes protobuf and sends a status. Use a
// loopback HTTP/2 peer for cases that need to violate those wire guarantees.
void SendWireResponse(int listener, WireEnding ending) {
  Socket connection(accept(listener, nullptr, nullptr));
  const auto stream = ReceiveRequest(connection.fd());
  // HPACK indexed :status 200, then literal content-type with an indexed name.
  SendFrame(connection.fd(), 1, 4, stream, "\x88\x0f\x10\x10" "application/grpc");
  std::string data;
  if (ending == WireEnding::Disconnect) {
    data = GrpcRecord(Heartbeat().SerializeAsString());
    data += GrpcRecord(Terminal(v1::EXECUTION_STATE_UNKNOWN, 2).SerializeAsString());
  } else if (ending != WireEnding::BadProtobuf) {
    data = GrpcRecord(Terminal(v1::EXECUTION_STATE_UNKNOWN).SerializeAsString());
  }
  if (ending == WireEnding::BadProtobuf || ending == WireEnding::ExtraBadProtobuf) {
    data += GrpcRecord("\x0a\x10");  // A truncated length-delimited protobuf field.
  }
  SendFrame(connection.fd(), 0, ending == WireEnding::MissingStatus ? 1 : 0, stream, data);
  if (ending == WireEnding::Ok || ending == WireEnding::BadProtobuf ||
      ending == WireEnding::ExtraBadProtobuf) {
    // HPACK literal grpc-status: 0, with END_HEADERS and END_STREAM.
    constexpr char ok_status[] = "\x00\x0b" "grpc-status\x01" "0";
    SendFrame(connection.fd(), 1, 5, stream, {ok_status, sizeof(ok_status) - 1});
  }
  shutdown(connection.fd(), SHUT_WR);
}

void CheckWireEnding(WireEnding ending) {
  Socket listener(socket(AF_INET, SOCK_STREAM, 0));
  const timeval timeout{3, 0};
  Require(setsockopt(listener.fd(), SOL_SOCKET, SO_RCVTIMEO, &timeout, sizeof(timeout)) == 0,
          "could not bound wire fixture read time");
  sockaddr_in address{};
  address.sin_family = AF_INET;
  address.sin_addr.s_addr = htonl(INADDR_LOOPBACK);
  Require(bind(listener.fd(), reinterpret_cast<const sockaddr*>(&address), sizeof(address)) == 0 &&
              listen(listener.fd(), 1) == 0,
          "could not start wire test listener");
  socklen_t length = sizeof(address);
  Require(getsockname(listener.fd(), reinterpret_cast<sockaddr*>(&address), &length) == 0,
          "could not determine wire test port");

  auto channel = Loopback::Channel(ntohs(address.sin_port));
  auto peer = std::async(std::launch::async, [&] { SendWireResponse(listener.fd(), ending); });
  sdk::TaskProgressReader reader(channel, Request(), Clock::now() + 3s);
  const auto first = reader.Next();
  const auto last = ending == WireEnding::Disconnect ? reader.Next() : first;
  peer.get();
  switch (ending) {
    case WireEnding::Ok:
      Require(std::holds_alternative<sdk::TaskUnknown>(std::get<sdk::TaskOutcome>(last)),
              "wire fixture did not deliver a terminal with OK status");
      break;
    case WireEnding::BadProtobuf:
      CheckError(last, sdk::ProgressError::MalformedFrame, "malformed protobuf accepted as EOF");
      break;
    case WireEnding::ExtraBadProtobuf:
      CheckError(last, sdk::ProgressError::TrailingFrame, "extra malformed protobuf accepted as EOF");
      break;
    case WireEnding::Disconnect:
      Require(std::holds_alternative<v1::Heartbeat>(first), "no progress before socket disconnect");
      [[fallthrough]];
    case WireEnding::MissingStatus: {
      const auto* failure = std::get_if<sdk::ProgressFailure>(&last);
      Require(failure && failure->reason == sdk::ProgressError::Transport &&
                  failure->transport_code != grpc::StatusCode::OK,
              "terminal confirmed without an OK status");
      break;
    }
  }
}

}  // namespace

bool CheckTaskWorkerProgress() {
  try {
    CheckEvents();
    CheckInvalidProgress();
    CheckCompletion();
    for (const auto ending : {WireEnding::Ok, WireEnding::BadProtobuf, WireEnding::ExtraBadProtobuf,
                              WireEnding::MissingStatus, WireEnding::Disconnect}) {
      CheckWireEnding(ending);
    }
    return true;
  } catch (const std::exception& error) {
    std::cerr << "TaskWorker progress loopback failed: " << error.what() << '\n';
    return false;
  }
}
