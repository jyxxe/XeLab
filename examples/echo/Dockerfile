FROM ubuntu:24.04
COPY target/debug/xelab-echo /usr/local/bin/xelab-echo
COPY examples/generated/xelab /xelab
USER 65532:65532
ENTRYPOINT ["/usr/local/bin/xelab-echo"]
