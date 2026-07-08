import socket, select, time, sys, traceback

s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("0.0.0.0", 7000))
s.listen(100)
print("Relay v2 OK :7000", flush=True)

w = None

while True:
    if w is None:
        w, _ = s.accept()
        print("W", flush=True)

    r, _, _ = select.select([s], [], [], 5)
    if not r:
        continue

    l, addr = s.accept()
    print(f"L {addr}", flush=True)
    try:
        w.setblocking(0)
        l.setblocking(0)
        while True:
            r, _, _ = select.select([w, l], [], [], 5)
            if not r:
                continue
            for sock in r:
                d = sock.recv(4096)
                if not d:
                    raise ConnectionError("closed")
                if sock is l:
                    w.sendall(d)
                    print(">", len(d), flush=True)
                else:
                    l.sendall(d)
                    print("<", len(d), flush=True)
    except Exception as e:
        print(f"X {e}", flush=True)
    try:
        l.close()
    except:
        pass
    try:
        w.close()
    except:
        pass
    w = None
