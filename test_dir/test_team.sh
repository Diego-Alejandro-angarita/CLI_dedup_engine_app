#!/bin/bash
set -e
export PATH=$(pwd)/../target/release:$PATH

echo "--- Unlocking TEAM Tier ---"
dedup-engine auth TEAM-SUPER-SECRET-KEY

echo -e "\n--- Creating Isolated Project (Backend Logs) ---"
mkdir -p backend_project
cd backend_project

cat << 'JSON' > dedup.json
{
  "compression": true,
  "watch": false,
  "path": "./isolated_backend_repo",
  "target": "backend.log"
}
JSON

echo "Simulating backend log day 1..." > backend.log
dedup-engine backup
echo "Simulating backend log day 2..." >> backend.log
dedup-engine backup

echo -e "\n--- Checking Stats in Backend Project ---"
dedup-engine stats

echo -e "\n--- Checking History in Backend Project ---"
dedup-engine history

cd ..

echo -e "\n--- Creating Isolated Project (Configs) ---"
mkdir -p config_project
cd config_project

cat << 'JSON' > dedup.json
{
  "compression": false,
  "path": "./isolated_config_repo",
  "target": "config.yaml"
}
JSON

echo "port: 8080" > config.yaml
dedup-engine backup

echo -e "\n--- Checking Stats in Config Project ---"
dedup-engine stats

echo -e "\n--- Checking History in Config Project ---"
dedup-engine history

cd ..

echo -e "\n--- Cleaning up ---"
rm -rf backend_project config_project ~/.dedup
