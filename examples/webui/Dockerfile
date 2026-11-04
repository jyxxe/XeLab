FROM ubuntu:24.04 AS bindings
RUN apt-get update && apt-get install -y --no-install-recommends protobuf-compiler protobuf-compiler-grpc && rm -rf /var/lib/apt/lists/*
COPY docs/contracts/proto /proto
RUN mkdir /bindings && protoc -I /proto --python_out=/bindings --grpc_python_out=/bindings --plugin=protoc-gen-grpc_python=/usr/bin/grpc_python_plugin /proto/xelab/v1/common.proto /proto/xelab/v1/lifecycle.proto

FROM ubuntu:24.04 AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends python3 python3-grpcio python3-protobuf && rm -rf /var/lib/apt/lists/*
COPY --from=bindings /bindings /opt/webui
COPY examples/webui/server.py /opt/webui/server.py
COPY examples/webui/public /opt/webui/public
ARG ARTIFACT_DIR=examples/webui/generated
COPY ${ARTIFACT_DIR}/xelab /xelab
RUN chmod -R a+rX /opt/webui /xelab
ENV PYTHONDONTWRITEBYTECODE=1
USER 65532:65532
EXPOSE 7000 8080
ENTRYPOINT ["/usr/bin/python3", "/opt/webui/server.py"]

FROM runtime AS component-tests
USER root
RUN apt-get update && apt-get install -y --no-install-recommends openssl && rm -rf /var/lib/apt/lists/*
COPY --chmod=0444 examples/webui/test_server.py /opt/webui/test_server.py
USER 65532:65532
ENTRYPOINT ["/usr/bin/python3", "/opt/webui/test_server.py", "--rpc-probe"]

FROM runtime AS webui
