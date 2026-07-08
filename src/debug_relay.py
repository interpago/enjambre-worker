import logging, socket, threading, os, sys
logging.basicConfig(level=logging.DEBUG, format="%(asctime)s [%(threadName)s] %(message)s", stream=sys.stdout)
log = logging.getLogger()

workers = {}
lock = threading.Lock()

def handle(c, a, p):
    log.info("handle_connection(%s, %d) iniciado", a, p)
    try:
        c.settimeout(5)
        m = c.recv(1)
        log.info("Primer byte: %s (0x%02x)", m, m[0] if m else 0)
        if not m or m[0] not in range(65, 91):
            log.warning("Byte invalido, cerrando")
            c.close()
            return
        if m == b"E":
            with lock:
                old = workers.get(p)
                if old:
                    log.info("Cerrando worker anterior en puerto %d", p)
                    try: old.close()
                    except: pass
                workers[p] = c
            log.info("Worker registrado desde %s en puerto %d", a, p)
            c.settimeout(None)
            log.info("Worker thread entrando a loop recv infinito")
            while True:
                d = c.recv(65536)
                if not d:
                    log.info("Worker recv: EOF - conexion cerrada por remoto")
                    break
                log.info("Worker recv: %d bytes (descartados)", len(d))
            log.info("Worker thread saliendo del loop")
        else:
            log.info("API request desde %s en puerto %d", a, p)
            with lock: w = workers.get(p)
            if not w:
                log.warning("No worker en puerto %d", p)
                c.close()
                return
            log.info("Enviando magic byte al worker")
            w.sendall(m)
            log.info("Iniciando bridge")
            bridge(c, w)
            log.info("Bridge completado")
            c.close()
    except Exception as e:
        log.error("Error en handle_connection(%s, %d): %s: %s", a, p, type(e).__name__, e)
    log.info("handle_connection(%s, %d) finalizando", a, p)

def bridge(api, worker):
    def f(s, d, name):
        try:
            while True:
                x = s.recv(65536)
                if not x:
                    log.info("Bridge %s: EOF", name)
                    break
                log.info("Bridge %s: %d bytes", name, len(x))
                d.sendall(x)
        except Exception as e:
            log.debug("Bridge %s error: %s", name, e)
        finally:
            try: d.close()
            except: pass
    t1 = threading.Thread(target=f, args=(api, worker, "api->worker"), daemon=True)
    t2 = threading.Thread(target=f, args=(worker, api, "worker->api"), daemon=True)
    t1.start(); t2.start()
    t1.join(); t2.join()

def serve(host, port):
    s = socket.socket()
    s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind((host, port))
    s.listen(128)
    log.info("Listening on %s:%d", host, port)
    while True:
        c, a = s.accept()
        log.info("Accept: %s -> %d", a, port)
        threading.Thread(target=handle, args=(c, a, port), daemon=True).start()

t1 = threading.Thread(target=serve, args=("0.0.0.0", 7000), daemon=True)
t1.start()
# Solo 7000 para prueba
t1.join()
