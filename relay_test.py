import socket, sys

s = socket.socket()
s.settimeout(15)
s.connect(("127.0.0.1", 7000))
body = b'{"model":"qwen-2.5-7b-coder","messages":[{"role":"user","content":"Hola"}],"max_tokens":20}'
req = (
    b"POST /v1/chat/completions HTTP/1.1\r\n"
    b"Host: localhost:8081\r\n"
    b"Content-Type: application/json\r\n"
    b"Content-Length: " + str(len(body)).encode() + b"\r\n"
    b"Connection: close\r\n\r\n" + body
)
s.sendall(req)
resp = b""
while True:
    try:
        d = s.recv(4096)
        if not d:
            break
        resp += d
    except:
        break
print(repr(resp[:1000]))
s.close()
