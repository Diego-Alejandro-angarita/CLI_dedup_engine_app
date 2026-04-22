#!/bin/bash
set -e
export PATH=../target/release:$PATH

echo "--- Testing Init and Pro Features ---"
dedup-engine backup dummy.txt --compress || true

echo -e "\n--- Creating Dummy File ---"
echo "Dummy content" > test_file.txt

echo -e "\n--- Testing Backup ---"
dedup-engine backup test_file.txt

echo -e "\n--- Testing Stats ---"
dedup-engine stats

echo -e "\n--- Testing Restore ---"
dedup-engine restore test_file.txt restored_file.txt
cat restored_file.txt

echo -e "\n--- Cleanup ---"
rm -rf ~/.dedup-engine test_file.txt restored_file.txt
