#!/bin/bash
cd /home/ubuntu
nohup python3 -u /home/ubuntu/tunnel-relay.py > /home/ubuntu/relay_out.log 2>&1 &
echo $!
