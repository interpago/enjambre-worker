import asyncio, logging
logging.basicConfig(level=logging.INFO, format="%(asctime)s %(message)s")

async def handler(reader, writer):
    peer = writer.get_extra_info("peername")
    logging.info(f"Conexion de {peer}")
    magic = await reader.readexactly(1)
    logging.info(f"Primer byte: {magic}")
    if magic == b"E":
        logging.info("Worker registrado - esperando...")
        await asyncio.sleep(9999)
    writer.close()

async def main():
    s = await asyncio.start_server(handler, "0.0.0.0", 7777)
    logging.info("Escuchando en puerto 7777")
    async with s:
        await s.serve_forever()

asyncio.run(main())
