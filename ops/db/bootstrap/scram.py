#!/usr/bin/env python3
import base64, hashlib, hmac, os, sys
pw = sys.stdin.readline().rstrip("\n").encode()
salt, iters = os.urandom(16), 4096
salted = hashlib.pbkdf2_hmac("sha256", pw, salt, iters)
stored = hashlib.sha256(hmac.new(salted, b"Client Key", "sha256").digest()).digest()
server = hmac.new(salted, b"Server Key", "sha256").digest()
b = lambda x: base64.b64encode(x).decode()
print(f"SCRAM-SHA-256${iters}:{b(salt)}${b(stored)}:{b(server)}")
