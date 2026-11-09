FROM ubuntu:24.04 AS bindings
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler protobuf-compiler-grpc && rm -rf /var/lib/apt/lists/*
COPY docs/contracts/proto /proto
RUN mkdir /bindings && protoc -I /proto --python_out=/bindings --grpc_python_out=/bindings --plugin=protoc-gen-grpc_python=/usr/bin/grpc_python_plugin /proto/xelab/v1/common.proto /proto/xelab/v1/lifecycle.proto /proto/xelab/v1/capability.proto

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-grpcio python3-protobuf && rm -rf /var/lib/apt/lists/*
COPY --from=bindings /bindings /opt/plugin
# Reuse the existing real, strictly authenticated lifecycle adapter, while the
# business/static application and signed manifest are this plugin's own files.
COPY examples/http/server.py /opt/plugin/server.py
COPY examples/ui-reference/http_server.py /opt/plugin/http_server.py
COPY examples/ui-reference/public /opt/plugin/public
ARG ARTIFACT_DIR=examples/ui-reference/generated
COPY ${ARTIFACT_DIR}/xelab /xelab
RUN chmod -R a+rX /opt/plugin /xelab
ENV PYTHONDONTWRITEBYTECODE=1
USER 65532:65532
ENTRYPOINT ["/usr/bin/python3", "/opt/plugin/server.py"]
