#!/usr/bin/env python3
"""Expose the WebRTC TURN relay through the router with NAT-PMP (RFC 6886).

WebRTC needs a *reachable* rendezvous. The signaling WebSocket is proxied over
TCP 443, but the data channel runs over UDP between the browser and the sim, so
a phone on mobile data (carrier-grade NAT) can only connect through a TURN
relay. A self-hosted coturn still needs its listener and relay range forwarded
from the router; this script asks the gateway to do that automatically and
keeps the mappings alive.

Mappings (external port == internal port):
  * 3478/udp + 3478/tcp   -> coturn TURN/STUN listener
  * 61000-61040/udp       -> coturn relay range (must match --min-port/--max-port;
                             above the Linux ephemeral range, so coturn can bind)

Usage:
  scripts/natpmp-turn.py             # map once, print the result
  scripts/natpmp-turn.py --loop      # refresh forever at 2/3 of the lifetime

Run it on the host (or in a `--network host` container) so the mappings point
at the machine running coturn. Routers that do not speak NAT-PMP (or with it
disabled) answer nothing: forward the ranges in the router UI instead.
"""
from __future__ import annotations

import argparse
import socket
import struct
import sys
import time

GATEWAY_FILE = "/proc/net/route"
NATPMP_PORT = 5351
OP_UDP, OP_TCP = 1, 2
RELAY_FIRST, RELAY_LAST = 61000, 61040
TURN_PORT = 3478


def gateway() -> str | None:
    """Default IPv4 gateway from /proc/net/route."""
    for line in open(GATEWAY_FILE, encoding="ascii"):
        fields = line.split()
        if len(fields) > 2 and fields[1] == "00000000":
            return socket.inet_ntoa(struct.pack("<L", int(fields[2], 16)))
    return None


def parse_range(text: str) -> tuple[int, int]:
    first, _, last = text.partition("-")
    first_i = int(first)
    return first_i, int(last or first)


def map_port(gw: str, opcode: int, internal: int, external: int, lifetime: int) -> dict:
    """One NAT-PMP mapping request. Response: version, op|0x80, result, epoch, internal, external, lifetime."""
    request = struct.pack("!BBHHH", 0, opcode, 0, internal, external) + struct.pack("!I", lifetime)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(2.0)
    try:
        sock.sendto(request, (gw, NATPMP_PORT))
        data, _ = sock.recvfrom(64)
    except OSError as error:
        return {"internal": internal, "ok": False, "error": str(error)}
    finally:
        sock.close()
    if len(data) < 16:
        return {"internal": internal, "ok": False, "error": "short reply"}
    result = struct.unpack("!H", data[2:4])[0]
    internal_port, external_port = struct.unpack("!HH", data[8:12])
    granted = struct.unpack("!I", data[12:16])[0]
    return {
        "internal": internal_port,
        "external": external_port,
        "lifetime": granted,
        "ok": result == 0,
        **({"error": f"NAT-PMP result code {result}"} if result else {}),
    }


def mappings(gw: str, relay: tuple[int, int], lifetime: int) -> list[tuple[int, int]]:
    jobs = [(OP_UDP, TURN_PORT), (OP_TCP, TURN_PORT)]
    jobs += [(OP_UDP, port) for port in range(relay[0], relay[1] + 1)]
    return [(op, internal) for op, internal in jobs]


def map_all(gw: str, relay: tuple[int, int], lifetime: int) -> int:
    """(Re)create every mapping. Returns the granted lifetime in seconds."""
    granted = lifetime
    failures = []
    for opcode, port in mappings(gw, relay, lifetime):
        reply = map_port(gw, opcode, port, port, lifetime)
        if reply["ok"]:
            granted = min(granted, reply["lifetime"] or lifetime)
        else:
            failures.append((opcode, port, reply.get("error", "?")))
    label = {"1": "udp", "2": "tcp"}
    print(
        f"natpmp: {len(mappings(gw, relay, lifetime)) - len(failures)}/"
        f"{len(mappings(gw, relay, lifetime))} mappings on {gw}, relay range "
        f"{relay[0]}-{relay[1]}, granted {granted}s",
        flush=True,
    )
    for opcode, port, error in failures[:8]:
        print(f"  failed {label[str(opcode)]}:{port}: {error}", flush=True)
    if len(failures) > 8:
        print(f"  … {len(failures) - 8} more failures", flush=True)
    return granted


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--loop", action="store_true", help="refresh forever (2/3 of the granted lifetime)")
    parser.add_argument("--gateway", help="router IPv4 (default: /proc/net/route)")
    parser.add_argument("--relay-range", default=f"{RELAY_FIRST}-{RELAY_LAST}", type=parse_range)
    parser.add_argument("--lifetime", type=int, default=7200, help="requested mapping lifetime, seconds")
    args = parser.parse_args()

    gw = args.gateway or gateway()
    if not gw:
        print("natpmp: no default gateway found", file=sys.stderr)
        return 1

    while True:
        granted = map_all(gw, args.relay_range, args.lifetime)
        if not args.loop:
            return 0
        # Refresh well before expiry; a router reboot changes the granted
        # lifetime (or the gateway), so re-discover both each round.
        time.sleep(max(60, int(granted * 2 / 3)))
        gw = args.gateway or gateway()
        if not gw:
            print("natpmp: default gateway disappeared; retrying", file=sys.stderr)
            time.sleep(30)
            gw = gateway() or "0.0.0.0"


if __name__ == "__main__":
    raise SystemExit(main())
