# Keywork Management Protocol v1

KWMP is transport-independent. USB carries complete frames over vendor bulk
endpoints; Bluetooth carries the same byte stream through a custom GATT
service. All integers are little-endian.

## Frame

| Offset | Size | Field |
|---:|---:|---|
| 0 | 4 | `KWMP` magic |
| 4 | 1 | protocol version (`1`) |
| 5 | 1 | flags |
| 6 | 2 | command |
| 8 | 4 | request ID |
| 12 | 4 | payload length |
| 16 | N | payload |

Responses use the same command and request ID. Their payload begins with a
`u16` management status.

## Core payloads

- `GetDeviceInfo`, `GetStatus`, `GetActiveProfile`: empty request.
- `Reboot`: `mode:u8` (`0` normal, `1` bootloader, `2` recovery).
- `SetActiveProfile`: `profile:u8`.
- `ReadProfile`: `profile:u8, offset:u32, maximum_length:u16`.
- `WriteProfile`: `profile:u8, offset:u32, flags:u8, data...`.
  - flag bit 0: truncate before this write.
  - flag bit 1: final chunk.

## Asset transfers

- `BeginAssetUpload`: `asset_id:u32, kind:u8, total_size:u64, sha256:[u8;32]`.
- `WriteAssetChunk`: `transfer_id:u32, offset:u64, data...`.
- `FinishAssetUpload`: `transfer_id:u32`.
- `DeleteAsset`: `asset_id:u32`.
- `BeginAssetDownload`: `asset_id:u32`.
- `ReadAssetChunk`: `transfer_id:u32, offset:u64, maximum_length:u16`.
- `FinishAssetDownload`: `transfer_id:u32`.

Begin responses return `transfer_id:u32, maximum_chunk_size:u16`. Asset download
also returns `total_size:u64, sha256:[u8;32]`.

## Firmware update

- `BeginFirmwareUpdate`:
  `image_size:u64, image_version:u32, sha256:[u8;32], signature_length:u16, signature...`.
- `WriteFirmwareChunk`: `transfer_id:u32, offset:u64, data...`.
- `FinishFirmwareUpdate`: `transfer_id:u32`.
- `AbortFirmwareUpdate`: `transfer_id:u32`.
- `GetFirmwareUpdateStatus`: empty request.
- `ConfirmRunningFirmware`: empty request.

The backend chooses the inactive A/B slot. Confirmation should normally be
performed internally by healthy newly-booted firmware; production builds may
reject external confirmation.
