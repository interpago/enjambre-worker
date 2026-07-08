import logging, socket, threading
logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")
workers = {}
lock = threading.Lock()

def bridge(api, worker):
    def f(s, d):
        try:
            while True:
                x = s.recv(65536)
                if not x: break
                d.sendall(x)
        except: pass
        finally:
            try: d.close()
            except: pass
    t1 = threading.Thread(target=f, args=(api, worker), daemon=True)
    t2 = threading.Thread(target=f, args=(worker, api), daemon=True)
    t1.start(); t2.start()
    t1.join(); t2.join()

def handle(c, a, p):
    try:
        c.settimeout(5)
        m = c.recv(1)
        if not m or m[0] not in range(65, 91):
            c.close()
            return
        if m == b"E":
            with lock:
                old = workers.get(p)
                if old:
                    try: old.close()
                    except: pass
                workers[p] = c
            logging.info("Worker from %s on port %d", a, p)
            c.settimeout(None)
            while True:
                d = c.recv(65536)
                if not d: break
        else:
            with lock: w = workers.get(p)
            if not w:
                c.close()
                return
            w.sendall(m)
            bridge(c, w)
            c.close()
    except: pass

def serve(host, port):
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((host, port))
    s.listen(128)
    logging.info("Listening on %s:%d", host, port)
    while True:
        c, a = s.accept()
        threading.Thread(target=handle, args=(c, a, port), daemon=True).start()

t1 = threading.Thread(target=serve, args=("0.0.0.0", 7000), daemon=True)
t2 = threading.Thread(target=serve, args=("0.0.0.0", 18001), daemon=True)
t1.start(); t2.start()
threading.Event().wait()
