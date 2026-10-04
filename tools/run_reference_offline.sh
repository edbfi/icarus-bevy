#!/usr/bin/env bash
# Approved offline laboratory execution only; never use this as a production launcher.
set -euo pipefail

project_root="$(pwd -P)"
case_root="$project_root/work/icarus-offline-startup"
server_image="ghcr.io/pelican-eggs/steamcmd@sha256:967e40780cb46a5343bd7a5fd372612d1a1b90fbc95fdd803d74ec5b3bede237"
server_id_file="$case_root/container.cid"

bash "$project_root/.agents/vendor/reverse-skill/skills/scripts/case-guard.sh" \
  --case-root "$case_root"
test ! -e "$server_id_file"
mkdir -p "$case_root/runtime" "$case_root/evidence"

cleanup() {
  if [[ -s "$server_id_file" ]]; then
    server_id="$(cat "$server_id_file")"
    docker logs "$server_id" > "$case_root/evidence/container.log" 2>&1 || true
    docker inspect "$server_id" > "$case_root/evidence/container-inspect.json" || true
    docker stop --time 10 "$server_id" > /dev/null 2>&1 || true
    docker inspect "$server_id" > "$case_root/evidence/container-after-stop.json" || true
    docker rm "$server_id" > /dev/null 2>&1 || true
  fi
}
trap cleanup EXIT

docker run --detach --pull never --cidfile "$server_id_file" \
  --network none --read-only --cap-drop ALL \
  --security-opt no-new-privileges \
  --memory 6g --cpus 2 --pids-limit 512 --shm-size 256m \
  --tmpfs /tmp:rw,exec,nosuid,size=1g --tmpfs /run:rw,nosuid,size=16m \
  --mount "type=bind,src=$project_root/icarus-orig,dst=/reference,readonly" \
  --mount "type=bind,src=$case_root/runtime,dst=/home/container" \
  --workdir /reference \
  --env SteamAppId=1149460 --env SteamGameId=1149460 \
  --env STEAM_COMPAT_DATA_PATH=/home/container/proton-prefix \
  --env STEAM_COMPAT_CLIENT_INSTALL_PATH=/home/container \
  "$server_image" /bin/bash -lc \
  'mkdir -p "$STEAM_COMPAT_DATA_PATH"; exec xvfb-run -a proton run /reference/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe -UserDir="Z:/home/container/Icarus" -Log -SteamServerName="icarus-research-offline" -PORT=17777 -QueryPort=27015' \
  > "$case_root/evidence/container-id.txt"

server_id="$(cat "$server_id_file")"
# Bound the observation period; record resource samples without host port exposure.
for sample_index in $(seq 1 6); do
  if [[ "$(docker inspect --format '{{.State.Running}}' "$server_id")" != true ]]; then
    break
  fi
  docker stats --no-stream --format '{{json .}}' "$server_id" \
    >> "$case_root/evidence/resource-samples.jsonl"
  sleep 5
done
