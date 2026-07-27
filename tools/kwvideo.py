#!/usr/bin/env python3

from __future__ import annotations

import argparse
import shutil
import subprocess
from pathlib import Path


BRAND = b"kwv1"
DEFAULT_SIZE = (1366, 768)
DEFAULT_FPS = 30
DEFAULT_CRF = 24
KEYFRAME_INTERVAL_SECONDS = 2


def parse_size(value: str) -> tuple[int, int]:
    try:
        width_text, height_text = value.lower().split("x", maxsplit=1)
        width = int(width_text)
        height = int(height_text)
    except (ValueError, TypeError) as error:
        raise argparse.ArgumentTypeError(
            "size must use WIDTHxHEIGHT, for example 1366x768"
        ) from error

    if width <= 0 or height <= 0:
        raise argparse.ArgumentTypeError(
            "video dimensions must be greater than zero"
        )

    if width & 1 or height & 1:
        raise argparse.ArgumentTypeError(
            "HEVC yuv420p dimensions must both be even"
        )

    return width, height


def build_video_filter(
    size: tuple[int, int],
    fit: str,
    fps: int,
) -> str:
    width, height = size

    if fit == "cover":
        resize = (
            f"scale={width}:{height}:force_original_aspect_ratio=increase,"
            f"crop={width}:{height}"
        )
    elif fit == "contain":
        resize = (
            f"scale={width}:{height}:force_original_aspect_ratio=decrease,"
            f"pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:black"
        )
    elif fit == "stretch":
        resize = f"scale={width}:{height}"
    else:
        raise ValueError(f"unsupported fit mode: {fit}")

    return f"{resize},fps={fps},format=yuv420p"


def validate_ftyp(output_path: Path) -> tuple[int, list[bytes]]:
    with output_path.open("rb") as file:
        header = file.read(16)

        if len(header) < 16:
            raise RuntimeError("FFmpeg output is too short to contain ftyp")

        box_size = int.from_bytes(header[0:4], "big")
        box_type = header[4:8]
        major_brand = header[8:12]
        minor_version = int.from_bytes(header[12:16], "big")

        if box_type != b"ftyp":
            raise RuntimeError(
                f"FFmpeg output does not begin with ftyp: {box_type!r}"
            )

        if box_size < 16 or box_size & 3:
            raise RuntimeError(f"invalid FFmpeg ftyp size: {box_size}")

        if major_brand != BRAND:
            raise RuntimeError(
                f"FFmpeg emitted major brand {major_brand!r}, expected {BRAND!r}"
            )

        compatible = []
        for _ in range((box_size - 16) // 4):
            brand = file.read(4)
            if len(brand) != 4:
                raise RuntimeError("truncated FFmpeg ftyp compatible-brand list")
            compatible.append(brand)

    return minor_version, compatible


def convert_video(
    input_path: Path,
    output_path: Path,
    size: tuple[int, int],
    fit: str,
    fps: int,
    crf: int,
) -> None:
    ffmpeg = shutil.which("ffmpeg")

    if ffmpeg is None:
        raise RuntimeError("ffmpeg was not found in PATH")

    if fps <= 0:
        raise ValueError("fps must be greater than zero")

    if not 0 <= crf <= 51:
        raise ValueError("CRF must be between 0 and 51")

    output_path.parent.mkdir(parents=True, exist_ok=True)

    keyint = fps * KEYFRAME_INTERVAL_SECONDS
    video_filter = build_video_filter(size, fit, fps)
    x265_params = ":".join(
        (
            "bframes=0",
            "open-gop=0",
            f"keyint={keyint}",
            f"min-keyint={keyint}",
            "scenecut=0",
        )
    )

    command = [
        ffmpeg,
        "-hide_banner",
        "-y",
        "-i",
        str(input_path),
        "-map",
        "0:v:0",
        "-an",
        "-sn",
        "-dn",
        "-vf",
        video_filter,
        "-c:v",
        "libx265",
        "-crf",
        str(crf),
        "-tag:v",
        "hvc1",
        "-x265-params",
        x265_params,
        "-movflags",
        "+empty_moov+default_base_moof+frag_keyframe",
        "-video_track_timescale",
        "90000",
        "-brand",
        BRAND.decode("ascii"),
        "-f",
        "mp4",
        str(output_path),
    ]

    subprocess.run(command, check=True)

    minor_version, compatible = validate_ftyp(output_path)
    compatible_text = ", ".join(
        brand.decode("ascii", errors="replace") for brand in compatible
    )

    print()
    print(f"Input:        {input_path}")
    print(f"Output:       {output_path}")
    print(f"Dimensions:   {size[0]}x{size[1]}")
    print(f"Frame rate:   {fps} fps CFR")
    print("Codec:        HEVC Main / hvc1 / yuv420p")
    print("B-frames:     disabled")
    print(f"Keyframes:    every {KEYFRAME_INTERVAL_SECONDS} seconds")
    print("MP4 layout:   fragmented (empty_moov + default_base_moof)")
    print(f"Major brand:  {BRAND.decode('ascii')}")
    print(f"Minor version: {minor_version:#x}")
    print(f"Compatible:   {compatible_text or '(none)'}")
    print(f"File size:    {output_path.stat().st_size} bytes")


def main() -> None:
    parser = argparse.ArgumentParser(
        description=(
            "Convert a video to the Keywork kwv1 profile. The output remains "
            "a standard fragmented HEVC MP4 with a custom ftyp major brand."
        )
    )

    parser.add_argument(
        "input",
        type=Path,
        help="source video accepted by FFmpeg",
    )

    parser.add_argument(
        "output",
        type=Path,
        help="destination .kwv file (ISO-BMFF/MP4 bytes)",
    )

    parser.add_argument(
        "--size",
        type=parse_size,
        default=DEFAULT_SIZE,
        help="output dimensions (default: 1366x768)",
    )

    parser.add_argument(
        "--fit",
        choices=("cover", "contain", "stretch"),
        default="cover",
        help="how to fit the source into the output dimensions",
    )

    parser.add_argument(
        "--fps",
        type=int,
        default=DEFAULT_FPS,
        help="constant output frame rate (default: 30)",
    )

    parser.add_argument(
        "--crf",
        type=int,
        default=DEFAULT_CRF,
        help="x265 CRF quality value, 0-51 (default: 24)",
    )

    arguments = parser.parse_args()

    convert_video(
        arguments.input,
        arguments.output,
        arguments.size,
        arguments.fit,
        arguments.fps,
        arguments.crf,
    )


if __name__ == "__main__":
    main()
