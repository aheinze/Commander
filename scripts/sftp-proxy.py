#!/usr/bin/env python3
"""Private loopback fault proxy for test-sftp.py; never intercepts host traffic."""
import asyncio
import json
from pathlib import Path
import sys


async def serve(backend, directory):
    mode = "forward"
    rate = 0
    serial = None
    connections = set()

    async def connection(reader, writer):
        if mode == "offline":
            writer.close()
            return
        upstream = None
        try:
            source, upstream = await asyncio.open_connection("127.0.0.1", backend)
            connections.update((writer, upstream))

            async def pump(source, destination):
                while mode != "offline":
                    while mode == "paused":
                        await asyncio.sleep(0.01)
                    data = await source.read(16384)
                    if not data:
                        return
                    while mode == "paused":
                        await asyncio.sleep(0.01)
                    if mode == "offline":
                        return
                    destination.write(data)
                    await destination.drain()
                    if rate:
                        await asyncio.sleep(len(data) / rate)

            tasks = [asyncio.create_task(pump(reader, upstream)),
                     asyncio.create_task(pump(source, writer))]
            await asyncio.wait(tasks, return_when=asyncio.FIRST_COMPLETED)
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
        except (ConnectionError, OSError):
            pass
        finally:
            for stream in (writer, upstream):
                if stream is not None:
                    connections.discard(stream)
                    stream.close()

    async def controls():
        nonlocal mode, rate, serial
        while True:
            try:
                request = json.loads((directory / "control.json").read_text())
                if request["serial"] != serial:
                    mode = request["mode"]
                    rate = request.get("rate", 0)
                    assert mode in ("forward", "paused", "offline")
                    assert isinstance(rate, int) and rate >= 0
                    serial = request["serial"]
                    if mode == "offline":
                        for stream in tuple(connections):
                            stream.close()
                    temporary = directory / "ack.tmp"
                    temporary.write_text(json.dumps({"serial": serial, "mode": mode}))
                    temporary.replace(directory / "ack.json")
            except FileNotFoundError:
                pass
            await asyncio.sleep(0.01)

    server = await asyncio.start_server(connection, "127.0.0.1", 0)
    (directory / "proxy.json").write_text(json.dumps({"port": server.sockets[0].getsockname()[1]}))
    async with server:
        await controls()


if __name__ == "__main__":
    asyncio.run(serve(int(sys.argv[1]), Path(sys.argv[2])))
