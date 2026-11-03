FROM ubuntu:24.04
ARG ARTIFACT_DIR=examples/generated
COPY target/debug/xelab-echo /usr/local/bin/xelab-echo
COPY ${ARTIFACT_DIR}/xelab /xelab
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/xelab-echo"]
