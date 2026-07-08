import socket, time
s = socket.socket()
s.settimeout(5)
s.connect(("127.0.0.1", 7777))
s.send(b"E")
print("E enviado, esperando 4s...")
time.sleep(4)
try:
    d = s.recv(1024)
    print(f"recv={len(d)}B")
except socket.timeout:
    print("TIMEOUT - conexion viva!")
s.close()
print("FIN")
