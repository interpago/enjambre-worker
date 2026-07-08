import logging
import socket
import threading

logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")

workers = {}  # port -> {'conn': socket, 'cv': Condition, 'api': (socket, addr) or None}
worker_lock = threading.Lock()

def forward(src, dst):
    try:
        while True:
            data = src.recv(65536)
            if not data:
                break
            dst.sendall(data)
    except Exception:
        pass

def bridge(conn1, conn2):
    t1 = threading.Thread(target=forward, args=(conn1, conn2), daemon=True)
    t2 = threading.Thread(target=forward, args=(conn2, conn1), daemon=True)
    t1.start()
    t2.start()
    t1.join()
    t2.join()

def handle_connection(conn, addr, port):
    try:
        conn.settimeout(5)
        magic = conn.recv(1)
        if not magic or (magic[0] not in range(65, 91)):
            conn.close()
            return
    except Exception:
        conn.close()
        return

    if magic == b"E":
        handle_worker(conn, addr, port)
    else:
        handle_api(magic, conn, addr, port)

def handle_worker(conn, addr, port):
    """Worker registration: blocks until an API request arrives, then bridges"""
    cv = threading.Condition(worker_lock)
    entry = {'conn': conn, 'addr': addr, 'cv': cv, 'api': None}

    with cv:
        old = workers.get(port)
        if old:
            try:
                old['conn'].close()
            except:
                pass
        workers[port] = entry
        logging.info("Worker registered: %s on port %d", addr, port)

        conn.settimeout(None)
        cv.wait()  # blocks until API request signals

        api = entry.pop('api', None)

    # At this point we hold NO lock. Bridge using worker conn + api conn.
    if api:
        api_conn, api_addr = api
        logging.info("Bridging %s -> %s on port %d", api_addr, addr, port)
        try:
            bridge(api_conn, conn)
            logging.info("Bridge completed on port %d", port)
        except Exception as e:
            logging.error("Bridge error on port %d: %s", port, e)
        finally:
            try:
                api_conn.close()
            except:
                pass
    else:
        logging.info("Worker disconnected without API request on port %d", port)

    # Cleanup
    with worker_lock:
        if workers.get(port) is entry:
            del workers[port]
    try:
        conn.close()
    except:
        pass

def handle_api(magic, conn, addr, port):
    """API request: sends magic byte to worker and signals it to bridge"""
    if port != 7000:
        logging.warning("API proxy only on port 7000, got %d", port)
        conn.close()
        return

    logging.info("API request from %s on port %d", addr, port)

    with worker_lock:
        entry = workers.get(port)
        if entry is None:
            logging.warning("No worker on port %d", port)
            conn.close()
            return
        cv = entry['cv']
        worker_conn = entry['conn']

        # Store API connection and signal worker
        entry['api'] = (conn, addr)

        try:
            worker_conn.sendall(magic)
        except Exception as e:
            logging.error("Failed to send magic to worker: %s", e)
            entry.pop('api', None)
            conn.close()
            return

        cv.notify()

    # API thread returns - bridge is handled by worker thread
    # conn will be closed by worker thread after bridge

def start_server(host, port):
    server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    server.bind((host, port))
    server.listen(128)
    logging.info("Listening on %s:%d", host, port)
    return server

def serve_port(host, port):
    server = start_server(host, port)
    while True:
        try:
            conn, addr = server.accept()
            threading.Thread(target=handle_connection, args=(conn, addr, port), daemon=True).start()
        except Exception as e:
            logging.error("Accept error on port %d: %s", port, e)

def main():
    ports = [7000] + list(range(18000, 18100))
    threads = []
    for port in ports:
        t = threading.Thread(target=serve_port, args=("0.0.0.0", port), daemon=True)
        t.start()
        threads.append(t)
    for t in threads:
        t.join()

if __name__ == "__main__":
    main()
