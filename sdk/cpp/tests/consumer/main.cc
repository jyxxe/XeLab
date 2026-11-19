#include <xelab/sdk.h>

#include <google/protobuf/descriptor.h>
#include <iostream>
#include <string_view>

bool CheckTaskWorkerProgress();

namespace {

bool CheckTaskWorkerBindings() {
  constexpr char kTaskWorker[] = "xelab.v1.TaskWorker";
  xelab::v1::TaskWorker::Service service;
  const auto* descriptor =
      google::protobuf::DescriptorPool::generated_pool()->FindServiceByName(kTaskWorker);
  if (!descriptor || std::string_view(xelab::v1::TaskWorker::service_full_name()) != kTaskWorker) {
    return false;
  }

  const auto* execute = descriptor->FindMethodByName("Execute");
  const auto* cancel = descriptor->FindMethodByName("Cancel");
  const auto* query = descriptor->FindMethodByName("Query");
  if (!execute || !cancel || !query ||
      execute->client_streaming() || !execute->server_streaming() ||
      cancel->client_streaming() || cancel->server_streaming() ||
      query->client_streaming() || query->server_streaming() ||
      execute->input_type() != xelab::v1::ExecuteTaskRequest::descriptor() ||
      execute->output_type() != xelab::v1::TaskProgress::descriptor() ||
      cancel->input_type() != xelab::v1::CancelTaskRequest::descriptor() ||
      cancel->output_type() != xelab::v1::Ack::descriptor() ||
      query->input_type() != xelab::v1::QueryTaskRequest::descriptor() ||
      query->output_type() != xelab::v1::TaskResult::descriptor()) {
    return false;
  }

  xelab::v1::ExecuteTaskRequest request;
  if (request.has_checkpoint_blob_id()) {
    return false;
  }
  request.set_checkpoint_blob_id("00000000-0000-4000-8000-000000000002");
  xelab::v1::ExecuteTaskRequest decoded_request;
  if (!decoded_request.ParseFromString(request.SerializeAsString()) ||
      !decoded_request.has_checkpoint_blob_id() ||
      decoded_request.checkpoint_blob_id() != request.checkpoint_blob_id()) {
    return false;
  }

  xelab::v1::TaskProgress progress;
  progress.set_fence(7);
  progress.set_sequence(2);
  progress.mutable_checkpoint()->set_blob_id(request.checkpoint_blob_id());
  xelab::v1::TaskProgress decoded_progress;
  return decoded_progress.ParseFromString(progress.SerializeAsString()) &&
         decoded_progress.fence() == 7 && decoded_progress.sequence() == 2 &&
         decoded_progress.body_case() == xelab::v1::TaskProgress::kCheckpoint &&
         decoded_progress.checkpoint().blob_id() == request.checkpoint_blob_id();
}

}  // namespace

int main() {
  if (!xelab::sdk::IsUuid("00000000-0000-4000-8000-000000000001")) {
    return 1;
  }
  if (!CheckTaskWorkerBindings()) {
    std::cerr << "TaskWorker SDK consumer check failed\n";
    return 1;
  }
  return CheckTaskWorkerProgress() ? 0 : 1;
}
