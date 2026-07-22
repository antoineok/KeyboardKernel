#!/usr/bin/env python3
"""Minimal Keywork Management Protocol client over USB bulk endpoints."""

from __future__ import annotations

import argparse
import hashlib
import struct
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    import usb.core
    import usb.util
except ImportError as error:
    raise SystemExit(
        "PyUSB is required. On Arch Linux: sudo pacman -S python-pyusb"
    ) from error

VID = 0xCAFE
PID = 0x4001
MANAGEMENT_INTERFACE = 2
ENDPOINT_OUT = 0x02
ENDPOINT_IN = 0x82
MAX_FRAME_SIZE = 4096

MAGIC = b"KWMP"
PROTOCOL_VERSION = 1
HEADER = struct.Struct("<4sBBHII")
FLAG_RESPONSE = 1 << 0
FLAG_ERROR = 1 << 1

GET_DEVICE_INFO = 0x0001
REBOOT = 0x0003
BEGIN_FIRMWARE_UPDATE = 0x1000
WRITE_FIRMWARE_CHUNK = 0x1001
FINISH_FIRMWARE_UPDATE = 0x1002
ABORT_FIRMWARE_UPDATE = 0x1003
GET_FIRMWARE_UPDATE_STATUS = 0x1004

STATUS_NAMES = {
    0x0000: "Ok",
    0x0001: "UnknownCommand",
    0x0002: "InvalidPayload",
    0x0003: "InvalidState",
    0x0004: "Unsupported",
    0x0010: "Busy",
    0x0011: "Timeout",
    0x0020: "NotFound",
    0x0021: "AlreadyExists",
    0x0022: "NoSpace",
    0x0023: "PermissionDenied",
    0x0030: "InvalidTransfer",
    0x0031: "InvalidOffset",
    0x0032: "ChecksumMismatch",
    0x0040: "InvalidFirmware",
    0x0041: "InvalidSignature",
    0x0042: "VersionRejected",
    0xFFFF: "InternalError",
}

PHASE_NAMES = {
    0: "Idle",
    1: "Preparing",
    2: "Receiving",
    3: "Verifying",
    4: "PendingReboot",
    5: "Failed",
}

SLOT_NAMES = {0: "A", 1: "B", 0xFF: "Unknown"}


@dataclass(frozen=True)
class Response:
    command: int
    request_id: int
    flags: int
    status: int
    body: bytes


class KeyworkClient:
    def __init__(self) -> None:
        self.device = usb.core.find(idVendor=VID, idProduct=PID)
        if self.device is None:
            raise RuntimeError("Keywork device cafe:4001 not found")

        try:
            self.device.get_active_configuration()
        except usb.core.USBError:
            self.device.set_configuration()

        if self.device.is_kernel_driver_active(MANAGEMENT_INTERFACE):
            self.device.detach_kernel_driver(MANAGEMENT_INTERFACE)

        usb.util.claim_interface(self.device, MANAGEMENT_INTERFACE)
        self.request_id = 1

    def close(self) -> None:
        usb.util.release_interface(self.device, MANAGEMENT_INTERFACE)
        usb.util.dispose_resources(self.device)

    def request(self, command: int, payload: bytes = b"", timeout_ms: int = 5000) -> Response:
        request_id = self.request_id
        self.request_id = (self.request_id + 1) & 0xFFFF_FFFF
        if self.request_id == 0:
            self.request_id = 1

        frame = HEADER.pack(
            MAGIC,
            PROTOCOL_VERSION,
            0,
            command,
            request_id,
            len(payload),
        ) + payload

        if len(frame) > MAX_FRAME_SIZE:
            raise ValueError(f"KWMP frame too large: {len(frame)} bytes")

        written = self.device.write(ENDPOINT_OUT, frame, timeout=timeout_ms)
        if written != len(frame):
            raise RuntimeError(f"short USB write: {written}/{len(frame)} bytes")

        raw = bytes(self.device.read(ENDPOINT_IN, MAX_FRAME_SIZE, timeout=timeout_ms))
        if len(raw) < HEADER.size + 2:
            raise RuntimeError(f"short KWMP response: {len(raw)} bytes")

        magic, version, flags, response_command, response_id, payload_length = HEADER.unpack_from(raw)
        if magic != MAGIC:
            raise RuntimeError(f"invalid KWMP magic: {magic!r}")
        if version != PROTOCOL_VERSION:
            raise RuntimeError(f"unsupported KWMP version: {version}")
        if not flags & FLAG_RESPONSE:
            raise RuntimeError("device frame is not marked as a response")
        if response_command != command:
            raise RuntimeError(
                f"command mismatch: requested 0x{command:04x}, received 0x{response_command:04x}"
            )
        if response_id != request_id:
            raise RuntimeError(
                f"request ID mismatch: requested {request_id}, received {response_id}"
            )
        if payload_length != len(raw) - HEADER.size:
            raise RuntimeError(
                f"payload length mismatch: header={payload_length}, received={len(raw) - HEADER.size}"
            )

        status = int.from_bytes(raw[HEADER.size : HEADER.size + 2], "little")
        body = raw[HEADER.size + 2 :]
        response = Response(response_command, response_id, flags, status, body)
        if status != 0 or flags & FLAG_ERROR:
            name = STATUS_NAMES.get(status, "Unknown")
            raise RuntimeError(f"KWMP command 0x{command:04x} failed: {name} (0x{status:04x})")
        return response


def decode_device_info(body: bytes) -> None:
    if len(body) != 40:
        raise RuntimeError(f"unexpected DeviceInfo length: {len(body)}")

    protocol, hardware = struct.unpack_from("<HH", body, 0)
    firmware, bootloader = struct.unpack_from("<II", body, 4)
    active_slot = body[12]
    profile_count = body[13]
    capabilities = struct.unpack_from("<Q", body, 16)[0]
    serial = body[24:40].split(b"\0", 1)[0].decode("ascii", errors="replace")

    print(f"Protocol:        {protocol}")
    print(f"Hardware rev:    {hardware}")
    print(f"Firmware:        0x{firmware:08x}")
    print(f"Bootloader:      0x{bootloader:08x}")
    print(f"Active slot:     {SLOT_NAMES.get(active_slot, hex(active_slot))}")
    print(f"Profiles:        {profile_count}")
    print(f"Capabilities:    0x{capabilities:016x}")
    print(f"Serial:          {serial}")


def decode_update_status(body: bytes) -> dict[str, int]:
    if len(body) != 24:
        raise RuntimeError(f"unexpected FirmwareUpdateStatus length: {len(body)}")
    phase, slot, last_error, transfer_id, received, total = struct.unpack("<BBHIQQ", body)
    print(f"Phase:           {PHASE_NAMES.get(phase, str(phase))}")
    print(f"Target slot:     {SLOT_NAMES.get(slot, hex(slot))}")
    print(f"Transfer ID:     {transfer_id}")
    print(f"Progress:        {received}/{total} bytes")
    print(f"Last error:      {STATUS_NAMES.get(last_error, 'Unknown')} (0x{last_error:04x})")
    return {
        "phase": phase,
        "slot": slot,
        "last_error": last_error,
        "transfer_id": transfer_id,
        "received": received,
        "total": total,
    }


def command_info(client: KeyworkClient, _args: argparse.Namespace) -> None:
    decode_device_info(client.request(GET_DEVICE_INFO).body)


def command_status(client: KeyworkClient, _args: argparse.Namespace) -> None:
    decode_update_status(client.request(GET_FIRMWARE_UPDATE_STATUS).body)


def command_firmware(client: KeyworkClient, args: argparse.Namespace) -> None:
    image = args.image.read_bytes()
    digest = hashlib.sha256(image).digest()
    print(f"Image:           {args.image}")
    print(f"Size:            {len(image)} bytes")
    print(f"SHA-256:         {digest.hex()}")

    begin_payload = struct.pack("<QI", len(image), args.version) + digest + struct.pack("<H", 0)
    begin = client.request(BEGIN_FIRMWARE_UPDATE, begin_payload, timeout_ms=10_000)
    if len(begin.body) != 7:
        raise RuntimeError(f"unexpected BeginFirmwareUpdate response length: {len(begin.body)}")

    transfer_id, maximum_chunk_size, target_slot = struct.unpack("<IHB", begin.body)
    print(f"Transfer ID:     {transfer_id}")
    print(f"Chunk size:      {maximum_chunk_size} bytes")
    print(f"Target slot:     {SLOT_NAMES.get(target_slot, hex(target_slot))}")

    try:
        offset = 0
        while offset < len(image):
            chunk = image[offset : offset + maximum_chunk_size]
            payload = struct.pack("<IQ", transfer_id, offset) + chunk
            client.request(WRITE_FIRMWARE_CHUNK, payload, timeout_ms=10_000)
            offset += len(chunk)
            percent = offset * 100 // len(image)
            print(f"\rUploading:       {offset}/{len(image)} bytes ({percent:3d}%)", end="", flush=True)
        print()

        client.request(FINISH_FIRMWARE_UPDATE, struct.pack("<I", transfer_id), timeout_ms=30_000)
        print("Firmware staged and verified.")
        decode_update_status(client.request(GET_FIRMWARE_UPDATE_STATUS).body)
    except Exception:
        try:
            client.request(ABORT_FIRMWARE_UPDATE, struct.pack("<I", transfer_id))
        except Exception:
            pass
        raise

    if args.reboot:
        print("Requesting commit and reboot...")
        client.request(REBOOT, b"\x00")
        print("Reboot accepted. USB will disconnect shortly.")
    else:
        print("Run again with --reboot, or send a normal reboot command, to activate it.")


def parse_int(value: str) -> int:
    return int(value, 0)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Keywork USB management client")
    subparsers = parser.add_subparsers(dest="command", required=True)

    info = subparsers.add_parser("info", help="read device information")
    info.set_defaults(handler=command_info)

    status = subparsers.add_parser("firmware-status", help="read firmware update status")
    status.set_defaults(handler=command_status)

    firmware = subparsers.add_parser("firmware", help="stage a kernel image")
    firmware.add_argument("image", type=Path)
    firmware.add_argument("--version", type=parse_int, default=0)
    firmware.add_argument("--reboot", action="store_true", help="commit and reboot after verification")
    firmware.set_defaults(handler=command_firmware)
    return parser


def main() -> int:
    args = build_parser().parse_args()
    client = KeyworkClient()
    try:
        args.handler(client, args)
    finally:
        client.close()
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except (RuntimeError, usb.core.USBError, OSError, ValueError) as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
