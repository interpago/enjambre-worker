import socket, time

# Conexion 1: registra worker y lo mantiene vivo
w = socket.socket()
w.settimeout(None)
w.connect(("127.0.0.1", 7000))
w.send(b"E")
print("Worker registrado")

# Conexion 2: API request
a = socket.socket()
a.settimeout(10)
a.connect(("127.0.0.1", 7000))
http = (
    b"POST /v1/chat/completions HTTP/1.1\r\n"
    b"Host: x\r\n"
    b"Content-Type: application/json\r\n"
    b"Content-Length: 92\r\n"
    b"Connection: close\r\n"
    b"\r\n"
    b'{"model":"qwen-2.5-7b-coder","messages":[{"role":"user","content":"Hola"}],"stream":false}'
)
a.sendall(http)
print("API request enviado, esperando respuesta...")

all_data = b""
while True:
    try:
        d = a.recv(65536)
        if not d:
            break
        all_data += d
        print(f"  +{len(d)}B (total={len(all_data)}B)")
    except socket.timeout:
        print("  TIMEOUT")
        break

print(f"Total: {len(all_data)}B")
if all_data:
    print(f"Respuesta:\n{all_data.decode(errors='replace')[:2000]}")

a.close()
# Keep worker alive a bit longer for relay to process
time.sleep(2)
w.close()
print("FIN")
