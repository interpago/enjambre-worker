import socket, sys, json

s = socket.socket()
s.settimeout(20)
s.connect(("127.0.0.1", 7000))
body = b'{"model":"qwen-2.5-7b-coder","messages":[{"role":"user","content":"Hola"}],"max_tokens":20}'
req = (
    b"POST /v1/chat/completions HTTP/1.1\r\n"
    b"Host: localhost:8081\r\n"
    b"Content-Type: application/json\r\n"
    b"Content-Length: " + str(len(body)).encode() + b"\r\n"
    b"Connection: close\r\n\r\n" + body
)
print(f"Sending {len(req)} bytes...")
s.sendall(req)
print("Sent, reading response...")
resp = b""
try:
    while True:
        d = s.recv(4096)
        if not d:
            break
        resp += d
        print(f"  Recv {len(d)} bytes: {d[:80]!r}")
except socket.timeout:
    print("Timeout!")
except Exception as e:
    print(f"Error: {e}")
print(f"\nTotal response ({len(resp)} bytes):")
print(resp[:2000].decode("utf-8", errors="replace"))
s.close()
