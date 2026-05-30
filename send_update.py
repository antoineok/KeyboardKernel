#!/usr/bin/env python3

import argparse
import struct
import time
import zlib

import serial

MAGIC = b"UPD1"
REPLY_MAGIC = b"URP1"

CMD_START = 0x01
CMD_CHUNK = 0x02
CMD_END = 0x03
CMD_ABORT = 0x04
CMD_COMMIT = 0x05

REPLY_READY = 0x10
REPLY_ACK = 0x11
REPLY_NACK = 0x12
REPLY_STAGED_OK = 0x13
REPLY_ABORTED = 0x14
REPLY_ERROR = 0x15

CHUNK_SIZE = 1024


def make_packet(cmd: int, seq: int, payload: bytes) -> bytes:
    crc = zlib.crc32(payload) & 0xFFFFFFFF
    return MAGIC + struct.pack("<BIII", cmd, seq, len(payload), crc) + payload


def read_reply(ser: serial.Serial):
    window = bytearray()

    while True:
        b = ser.read(1)

        if not b:
            raise TimeoutError("timeout waiting for reply")

        window += b

        if len(window) > 4:
            window = window[-4:]

        if bytes(window) == REPLY_MAGIC:
            break

    status_raw = ser.read(1)
    value_raw = ser.read(4)

    if len(status_raw) != 1 or len(value_raw) != 4:
        raise TimeoutError("incomplete reply")

    status = status_raw[0]
    value = struct.unpack("<I", value_raw)[0]

    return status, value


def enter_update_mode(ser: serial.Serial):
    ser.write(b":update\r")
    ser.flush()

    line = ser.readline().decode(errors="replace").strip()

    if line != "UPDATE MODE":
        raise RuntimeError(f"failed to enter update mode, got: {line!r}")

    print("Entered update mode")


def send_image(port: str, baud: int, image_path: str, commit: bool, follow: bool):
    with open(image_path, "rb") as f:
        image = f.read()

    print(f"Image: {image_path}")
    print(f"Size: {len(image)} bytes")

    with serial.Serial(port, baudrate=baud, timeout=10) as ser:
        time.sleep(0.2)
        ser.reset_input_buffer()
        ser.reset_output_buffer()

        enter_update_mode(ser)

        print("Sending START")
        ser.write(make_packet(CMD_START, 0, struct.pack("<I", len(image))))
        ser.flush()

        status, value = read_reply(ser)
        print(f"START reply: status=0x{status:02x}, value={value}")

        if status != REPLY_READY:
            raise RuntimeError("START failed")

        seq = 0

        for off in range(0, len(image), CHUNK_SIZE):
            chunk = image[off:off + CHUNK_SIZE]

            ser.write(make_packet(CMD_CHUNK, seq, chunk))
            ser.flush()

            status, value = read_reply(ser)

            if status != REPLY_ACK or value != seq:
                raise RuntimeError(
                    f"bad ACK: expected seq={seq}, got status=0x{status:02x}, value={value}"
                )

            print(f"ACK seq={seq} {off + len(chunk)}/{len(image)}")
            seq += 1

        print("Sending END")
        ser.timeout = 120
        ser.write(make_packet(CMD_END, seq, b""))
        ser.flush()

        while True:
            status, value = read_reply(ser)
            print(f"END reply: status=0x{status:02x}, value={value}")
            if status == REPLY_STAGED_OK:
                break
            elif status == 0x16:
                continue
            elif status == REPLY_ERROR:
                raise RuntimeError(f"END failed error={value}")

        if commit:
            print("Sending COMMIT")
            ser.write(make_packet(CMD_COMMIT, seq + 1, b""))
            ser.flush()

            try:
                status, value = read_reply(ser)
                print(f"COMMIT reply: status=0x{status:02x}, value={value}")
            except TimeoutError:
                print("No COMMIT reply, probably rebooted")
        
        if follow:
            print("Following CM4 logs. Press Ctrl+C to stop.")
            ser.timeout = 1
            while True:
                try:
                    data = ser.read(1024)

                    if data:
                        print(data.decode(errors="replace"), end="", flush=True)

                except KeyboardInterrupt:
                    print("\nStopped log follow.")
                    break


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("port")
    parser.add_argument("image")
    parser.add_argument("--baud", type=int, default=115200)
    parser.add_argument("--commit", action="store_true")
    parser.add_argument("--follow", action="store_true", help="follow logs after commit")
    args = parser.parse_args()

    send_image(args.port, args.baud, args.image, args.commit, args.follow)


if __name__ == "__main__":
    main()