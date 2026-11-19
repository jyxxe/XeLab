#ifndef XELAB_SDK_H
#define XELAB_SDK_H

#include <chrono>
#include <cstdint>
#include <memory>
#include <optional>
#include <string>
#include <string_view>
#include <variant>

#include <grpcpp/grpcpp.h>

#include "xelab/v1/capability.grpc.pb.h"
#include "xelab/v1/host.grpc.pb.h"
#include "xelab/v1/lifecycle.grpc.pb.h"
#include "xelab/v1/task.grpc.pb.h"

namespace xelab::sdk {

inline constexpr int kMaxMessageBytes = 20 * 1024 * 1024;
inline constexpr char kPluginAddress[] = "0.0.0.0:7000";
using Deadline = std::chrono::system_clock::time_point;

struct RuntimeIdentity {
  std::string deployment_id;
  std::string instance_id;
  uint64_t generation;

  static RuntimeIdentity FromEnvironment();
  bool Matches(const v1::InitializeRequest& request) const;
};

// Adds the fixed workload listener and bounds gRPC message sizes.
void ConfigurePluginServer(grpc::ServerBuilder& builder);

// Dials the injected HostService address with workload mTLS.
std::unique_ptr<v1::HostService::Stub> ConnectHost();

Deadline CallDeadline(const grpc::ServerContext& call, const v1::RpcContext& context,
                      std::chrono::milliseconds method_limit);
std::unique_ptr<grpc::ClientContext> HostCallContext(const grpc::ServerContext& parent,
                                                    const v1::RpcContext& context,
                                                    std::chrono::milliseconds method_limit);
std::optional<v1::ErrorDetail> ReadErrorDetail(const grpc::ClientContext& call);

struct TaskSucceeded {
  std::string output_json;
};
struct TaskFailed {
  v1::ErrorDetail error;
};
struct TaskCanceled {};
// The worker reports that the execution outcome is still unknown.
struct TaskUnknown {};
using TaskOutcome = std::variant<TaskSucceeded, TaskFailed, TaskCanceled, TaskUnknown>;

enum class ProgressError {
  InvalidCall,
  MalformedFrame,
  WrongIdentity,
  WrongSequence,
  MissingBody,
  InvalidHeartbeat,
  InvalidCheckpoint,
  InvalidTerminal,
  MissingTerminal,
  TrailingFrame,
  DeadlineExceeded,
  Transport,
};

// A failure leaves execution unconfirmed. It does not establish that the
// worker did not execute or that resubmitting is safe.
struct ProgressFailure {
  ProgressError reason;
  grpc::StatusCode transport_code = grpc::StatusCode::OK;
};

struct EndOfProgress {};
using ProgressRead = std::variant<v1::Heartbeat, v1::Checkpoint, TaskOutcome,
                                  EndOfProgress, ProgressFailure>;

// A blocking, single-call reader for TaskWorker.Execute. Use a trusted channel
// with kMaxMessageBytes transport limits and gRPC retries disabled. The caller
// supplies the absolute deadline of the execution. A shorter request deadline
// or the Execute method limit also bounds the call.
class TaskProgressReader {
 public:
  TaskProgressReader(std::shared_ptr<grpc::ChannelInterface> channel,
                     const v1::ExecuteTaskRequest& request, Deadline deadline);
  ~TaskProgressReader();
  TaskProgressReader(const TaskProgressReader&) = delete;
  TaskProgressReader& operator=(const TaskProgressReader&) = delete;

  // Only a confirmed terminal outcome is followed by EndOfProgress. Failures
  // are sticky. Validate the reported JSON, errors and checkpoint against the
  // execution contract before using them. Call from a single thread at a time.
  ProgressRead Next();

 private:
  class Stream;
  ProgressFailure Fail(ProgressError reason,
                       grpc::StatusCode code = grpc::StatusCode::OK);
  bool Matches(std::string_view execution_id, uint64_t fence) const;

  std::unique_ptr<Stream> stream_;
  Deadline deadline_;
  std::string execution_id_;
  uint64_t fence_;
  uint64_t next_sequence_ = 1;
  std::optional<ProgressFailure> failure_;
};

bool IsUuid(std::string_view value);
std::string ReadManifest();
std::string Digest(std::string_view bytes);

}  // namespace xelab::sdk

#endif
