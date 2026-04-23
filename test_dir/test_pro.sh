#!/bin/bash
set -e
export PATH=../target/release:$PATH

echo "--- Generating dummy file ---"
echo "Pro user test line 1" > pro_file.txt

echo "--- Unlocking PRO ---"
dedup-engine auth MY-SECRET-KEY-123

echo -e "\n--- Testing Backup with Compression ---"
dedup-engine backup pro_file.txt --compress

echo -e "\n--- Simulating new lines and testing Watch Mode (in background) ---"
dedup-engine watch pro_file.txt --compress &
WATCH_PID=$!
sleep 2

echo "Appending to file..."
echo "New error line 2" >> pro_file.txt
sleep 3
echo "Appending to file again..."
echo "New info line 3" >> pro_file.txt
sleep 3

kill $WATCH_PID

echo -e "\n--- Testing Advanced Stats ---"
dedup-engine stats

echo -e "\n--- Testing Restore of Compressed backup ---"
# Check the last recipe created
RECIPE=$(ls -t ~/.dedup/recipes/ | head -n 1)
dedup-engine restore ${RECIPE} restored_pro_file.txt
cat restored_pro_file.txt

rm pro_file.txt restored_pro_file.txt
