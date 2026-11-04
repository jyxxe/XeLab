FROM ubuntu:24.04 AS bindings
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler protobuf-compiler-grpc && rm -rf /var/lib/apt/lists/*
COPY docs/contracts/proto /proto
RUN mkdir /bindings && protoc -I /proto --python_out=/bindings --grpc_python_out=/bindings --plugin=protoc-gen-grpc_python=/usr/bin/grpc_python_plugin /proto/xelab/v1/common.proto /proto/xelab/v1/lifecycle.proto

FROM ubuntu:24.04
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-grpcio python3-protobuf && rm -rf /var/lib/apt/lists/*
COPY --from=bindings /bindings /opt/plugin
COPY examples/http/server.py examples/http/http_server.py examples/http/index.html examples/http/lifecycle_probe.py /opt/plugin/
ARG ARTIFACT_DIR=examples/http/generated
COPY ${ARTIFACT_DIR}/xelab /xelab
RUN chmod -R a+rX /opt/plugin /xelab
ENV PYTHONDONTWRITEBYTECODE=1
USER 65532:65532
ENTRYPOINT ["/usr/bin/python3", "/opt/plugin/server.py"]
