# Capture sidecar for tools/run_reference_connected.py (connected-scenarios runbook).
# Build locally and bind the resulting image ID as the session's capture_image:
#   docker build --pull=false -t icarus-research-capture -f tools/capture.Dockerfile tools
# tcpdump drops to the uid 1000 user so captures stay owned by the host user.
FROM alpine@sha256:294b683cb724975bec92580e1e685676bd4b50bda910ddb8c51d4cabeaec77e6
RUN apk add --no-cache tcpdump && adduser -D -H -u 1000 capture
