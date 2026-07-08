import socket, time
s = socket.socket()
s.settimeout(10)
try:
    s.connect(("127.0.0.1", 7000))
    s.send(b"E")
    print("Enviado E, esperando 3s...")
    time.sleep(3)
    s.send(b"X")
    print("Enviado X")
    d = s.recv(1024)
    print(f"recv={len(d)}B data={d!r}")
except socket.timeout:
    print("TIMEOUT - conexion viva")
except Exception as e:
    print(f"ERROR: {e}")
finally:
    s.close()
print("FIN")
