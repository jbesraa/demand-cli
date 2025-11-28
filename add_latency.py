#!/usr/bin/env python3

import argparse
import asyncio
import statistics
import time
from dataclasses import dataclass
from typing import Tuple

# ------------------------------- CLI ---------------------------------

@dataclass(frozen=True)
class Route:
    one_way: float
    listen: Tuple[str, int]
    target: Tuple[str, int]

def _parse_hostport(s: str) -> Tuple[str, int]:
    # Accept "host:port" (IPv6 like "[::1]:20000" also supported)
    if s.startswith('['):  # IPv6 in brackets
        try:
            host, rest = s[1:].split(']', 1)
            assert rest.startswith(':'), "missing port separator ':' after IPv6 bracket"
            port = int(rest[1:])
            return host, port
        except Exception as e:
            raise argparse.ArgumentTypeError(f"invalid address '{s}': {e}")
    try:
        host, port_s = s.rsplit(':', 1)
        port = int(port_s)
    except Exception as e:
        raise argparse.ArgumentTypeError(f"invalid address '{s}': {e}")
    if not (0 < port < 65536):
        raise argparse.ArgumentTypeError(f"port out of range in '{s}'")
    return host, port

def _parse_route(arg: str) -> Route:
    # LATENCY,LISTEN_HOST:LISTEN_PORT,TARGET_HOST:TARGET_PORT
    try:
        latency_s, listen_s, target_s = [p.strip() for p in arg.split(',', 2)]
    except ValueError:
        raise argparse.ArgumentTypeError(
            "route must be LATENCY,LISTEN_HOST:LISTEN_PORT,TARGET_HOST:TARGET_PORT"
        )
    try:
        one_way = float(latency_s)
    except ValueError:
        raise argparse.ArgumentTypeError(f"invalid latency '{latency_s}'")
    if one_way < 0:
        raise argparse.ArgumentTypeError("latency must be ≥ 0")
    listen = _parse_hostport(listen_s)
    target = _parse_hostport(target_s)
    return Route(one_way=one_way, listen=listen, target=target)

def parse_args() -> argparse.Namespace:
    default_routes = [
        "0.250,127.0.0.1:20000,127.0.0.1:19000",
        "0.250,127.0.0.1:21000,127.0.0.1:18000",
        "0.250,127.0.0.1:22000,127.0.0.1:17000",
    ]
    epilog = (
        "Examples:\n"
        "  Run the three defaults explicitly:\n"
        "    python latency_proxy.py \\\n"
        "      --route 0.250,127.0.0.1:20000,127.0.0.1:19000 \\\n"
        "      --route 0.250,127.0.0.1:21000,127.0.0.1:18000 \\\n"
        "      --route 0.250,127.0.0.1:22000,127.0.0.1:17000\n"
        "\n"
        "  Single route with 120 ms one-way, listening on all interfaces:\n"
        "    python latency_proxy.py --route 0.120,0.0.0.0:20000,127.0.0.1:19000\n"
    )
    parser = argparse.ArgumentParser(
        description="TCP latency proxy for one or more routes (one-way delay).",
        epilog=epilog,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "-r", "--route",
        dest="routes",
        metavar="LATENCY,LISTEN_HOST:LISTEN_PORT,TARGET_HOST:TARGET_PORT",
        action="append",
        type=_parse_route,
        help="Add a latency route. Repeat to run multiple routes.",
    )
    parser.add_argument(
        "--window-report",
        type=int,
        default=500,
        help="Number of packets per stats window for periodic reporting (default: 500).",
    )
    parser.add_argument(
        "--read-chunk",
        type=int,
        default=65536,
        help="Socket read chunk size in bytes (default: 65536).",
    )
    args = parser.parse_args()

    if not args.routes:
        # Use the three defaults if none were provided
        args.routes = [_parse_route(s) for s in default_routes]
    return args

# ----------------------------- Proxy core -----------------------------

async def forward(source, dest, *, tag: str, one_way: float, read_chunk: int, window_report: int):
    from collections import deque
    q = deque()
    done = asyncio.Event()
    delays = []

    async def reader():
        while True:
            data = await source.read(read_chunk)
            now = time.perf_counter()
            if not data:
                done.set()
                break
            release = now + one_way
            q.append((now, release, data))

    async def writer():
        while True:
            while not q:
                if done.is_set():
                    try:
                        dest.write_eof()
                    except Exception:
                        pass
                    return
                await asyncio.sleep(0)
            arrival, release, data = q[0]
            delay = release - time.perf_counter()
            if delay > 0:
                await asyncio.sleep(delay)
            q.popleft()
            t_send = time.perf_counter()
            dest.write(data)
            await dest.drain()
            delays.append(t_send - arrival)
            if window_report > 0 and len(delays) % window_report == 0:
                window = delays[-window_report:]
                mean = statistics.mean(window)
                p95 = sorted(window)[int(0.95 * len(window))]
                print(f"[{tag}] one-way mean={mean:.3f}s p95={p95:.3f}s (target {one_way:.3f}s)")

    await asyncio.gather(reader(), writer())

async def handle_connection(client_r, client_w, *, route: Route, read_chunk: int, window_report: int):
    th, tp = route.target
    sh, sp = route.listen
    try:
        server_r, server_w = await asyncio.open_connection(th, tp)
    except Exception as e:
        addr = f"{th}:{tp}"
        print(f"[{sh}:{sp}] failed to connect to target {addr}: {e}")
        client_w.close()
        await client_w.wait_closed()
        return
    tag_cs = f"{sh}:{sp} client→server"
    tag_sc = f"{sh}:{sp} server→client"
    await asyncio.gather(
        forward(client_r, server_w, tag=tag_cs, one_way=route.one_way, read_chunk=read_chunk, window_report=window_report),
        forward(server_r, client_w, tag=tag_sc, one_way=route.one_way, read_chunk=read_chunk, window_report=window_report),
    )

async def serve_route(route: Route, *, read_chunk: int, window_report: int):
    sh, sp = route.listen
    th, tp = route.target
    server = await asyncio.start_server(
        lambda r, w: handle_connection(r, w, route=route, read_chunk=read_chunk, window_report=window_report),
        sh, sp
    )
    addrs = ", ".join(str(sock.getsockname()) for sock in server.sockets or [])
    print(f"[listen {addrs}] → target {th}:{tp}  one-way delay {route.one_way:.3f}s")
    async with server:
        await server.serve_forever()

async def main_async():
    args = parse_args()
    tasks = [
        asyncio.create_task(serve_route(route, read_chunk=args.read_chunk, window_report=args.window_report))
        for route in args.routes
    ]
    await asyncio.gather(*tasks)

def main():
    try:
        asyncio.run(main_async())
    except KeyboardInterrupt:
        pass

if __name__ == "__main__":
    main()

