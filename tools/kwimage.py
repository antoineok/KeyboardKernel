#!/usr/bin/env python3

from __future__ import annotations

import argparse
import struct
from pathlib import Path

from PIL import Image, ImageOps


MAGIC = b"KWIM"
VERSION = 1
HEADER_SIZE = 32

FORMAT_RGBX8888 = 1
FORMAT_RGBA8888 = 3


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
            "image dimensions must be greater than zero"
        )

    return width, height


def resize_image(
    image: Image.Image,
    size: tuple[int, int] | None,
    fit: str,
) -> Image.Image:
    image = image.convert("RGBA")

    if size is None:
        return image

    if fit == "cover":
        return ImageOps.fit(
            image,
            size,
            method=Image.Resampling.LANCZOS,
        )

    if fit == "contain":
        contained = ImageOps.contain(
            image,
            size,
            method=Image.Resampling.LANCZOS,
        )

        canvas = Image.new("RGBA", size, (0, 0, 0, 0))
        position = (
            (size[0] - contained.width) // 2,
            (size[1] - contained.height) // 2,
        )

        canvas.alpha_composite(contained, position)
        return canvas

    if fit == "stretch":
        return image.resize(
            size,
            resample=Image.Resampling.LANCZOS,
        )

    raise ValueError(f"unsupported fit mode: {fit}")


def convert_image(
    input_path: Path,
    output_path: Path,
    size: tuple[int, int] | None,
    fit: str,
    preserve_alpha: bool,
) -> None:
    with Image.open(input_path) as source:
        image = resize_image(source, size, fit)

    width, height = image.size
    pitch = width * 4

    if preserve_alpha:
        pixel_format = FORMAT_RGBA8888
        pixel_format_name = "RGBA8888"
        pixels = image.convert("RGBA").tobytes("raw", "RGBA")
    else:
        pixel_format = FORMAT_RGBX8888
        pixel_format_name = "RGBX8888"
        pixels = image.convert("RGB").convert("RGBA").tobytes("raw", "RGBA")

    data_length = len(pixels)

    expected_length = pitch * height
    if data_length != expected_length:
        raise RuntimeError(
            f"unexpected pixel length: got {data_length}, "
            f"expected {expected_length}"
        )

    header = struct.pack(
        "<4sHHBBHIIIII",
        MAGIC,
        VERSION,
        HEADER_SIZE,
        pixel_format,
        0,  # Flags.
        0,  # Reserved.
        width,
        height,
        pitch,
        data_length,
        0,  # Reserved.
    )

    if len(header) != HEADER_SIZE:
        raise RuntimeError(
            f"internal header size mismatch: {len(header)}"
        )

    output_path.parent.mkdir(parents=True, exist_ok=True)

    with output_path.open("wb") as output:
        output.write(header)
        output.write(pixels)

    print(f"Input:       {input_path}")
    print(f"Output:      {output_path}")
    print(f"Dimensions:  {width}x{height}")
    print(f"Pixel format: {pixel_format_name}")
    print(f"Pitch:       {pitch} bytes")
    print(f"Data:        {data_length} bytes")
    print(f"Total:       {HEADER_SIZE + data_length} bytes")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Convert an image to the native Keywork image format."
    )

    parser.add_argument(
        "input",
        type=Path,
        help="source PNG, JPEG, WebP, or other Pillow-supported image",
    )

    parser.add_argument(
        "output",
        type=Path,
        help="destination .kwim file",
    )

    parser.add_argument(
        "--size",
        type=parse_size,
        help="output dimensions, for example 1366x768",
    )

    parser.add_argument(
        "--fit",
        choices=("cover", "contain", "stretch"),
        default="cover",
        help="how to fit the image when --size is supplied",
    )

    parser.add_argument(
        "--alpha",
        action="store_true",
        help="preserve transparency and emit RGBA8888 instead of RGBX8888",
    )

    arguments = parser.parse_args()

    convert_image(
        arguments.input,
        arguments.output,
        arguments.size,
        arguments.fit,
        arguments.alpha,
    )


if __name__ == "__main__":
    main()