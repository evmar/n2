#!/bin/sh

for i in $(seq 3); do
    echo $1: $i
    # Uncomment me to verify which fds are open:
    lsof -a -p $$
    sleep 1
done
