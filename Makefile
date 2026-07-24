SHELL := /bin/bash
.SHELLFLAGS := -eu -o pipefail -c
.ONESHELL:

.DEFAULT_GOAL := build-img

# =============================================================================
# Tools
#
# These are provided by the Nix development environment. Variables remain
# overridable for convenience, but the Makefile does not try to locate or
# install them.
# =============================================================================

CARGO       ?= cargo
OBJCOPY     ?= aarch64-none-elf-objcopy
QEMU        ?= qemu-system-aarch64
OPENOCD     ?= openocd
RUST_GDB    ?= aarch64-none-elf-gdb
TMUX_EXE    ?= tmux
PYTHON      ?= python
RPIBOOT     ?= rpiboot
UDISKSCTL   ?= udisksctl

# =============================================================================
# Kernel build
# =============================================================================

TARGET      ?= aarch64-unknown-none
KERNEL_NAME ?= kernel
KERNEL_IMG  ?= kernel8.img

RELEASE_PROFILE ?= release
JTAG_PROFILE    ?= gdb_release

RELEASE_ELF := target/$(TARGET)/$(RELEASE_PROFILE)/$(KERNEL_NAME)
JTAG_ELF    := target/$(TARGET)/$(JTAG_PROFILE)/$(KERNEL_NAME)

# Both normal and JTAG builds produce this same file:
#
#   kernel8.img
#
# Building either variant replaces the previous image.

# =============================================================================
# Hardware description
# =============================================================================

# Real CM4 device tree, also supplied to QEMU.
DTB ?= bcm2711-rpi-cm4.dtb

# =============================================================================
# QEMU
# =============================================================================

QEMU_MACHINE ?= raspi4b

# QEMU-only dummy eMMC backing image. It exists solely so that the emulated
# eMMC controller has a device attached.
QEMU_EMMC_IMG  ?= emmc.img
QEMU_EMMC_SIZE ?= 8G

UART_LOG ?= pl011_uart.log

QEMU_ARGS = \
	-M "$(QEMU_MACHINE)" \
	-kernel "$(KERNEL_IMG)" \
	-dtb "$(DTB)" \
	-drive file="$(QEMU_EMMC_IMG)",if=none,format=raw,id=emmc-img \
	-device emmc,drive=emmc-img,bus=/generic-sdhci/sd-bus \
	-chardev file,id=pl011,path="$(UART_LOG)" \
	-serial chardev:pl011 \
	-serial stdio

# =============================================================================
# Keywork deployment
# =============================================================================

KEYWORK_TOOL ?= tools/keywork.py

# =============================================================================
# GDB frontend
# =============================================================================

GDBGUI_PORT        ?= 5000
GDBGUI_HOST        ?= 127.0.0.1
GDBGUI_LISTEN      ?= 127.0.0.1
GDBGUI_TERMINAL_ID ?= keywork-gdb

# =============================================================================
# Legacy rpiboot deployment
# =============================================================================

CM4_BOOT_LABEL ?= bootfs
RPIBOOT_LOG    ?= /tmp/rpiboot.log
DEPLOY_TIMEOUT ?= 60

# =============================================================================
# Serial console
# =============================================================================

SERIAL_PORT ?= /dev/ttyACM0
SERIAL_BAUD ?= 115200

# =============================================================================
# Phony targets
# =============================================================================

.PHONY: \
	build-img \
	build-img-jtag \
	run-qemu \
	run-qemu-jtag \
	deploy \
	deploy-jtag \
	deploy-rpiboot \
	deploy-rpiboot-jtag \
	clean \
	serial \
	openocd \
	gdb \
	gdbgui \
	help

# =============================================================================
# Build
# =============================================================================

build-img:
	$(CARGO) build \
		--target "$(TARGET)" \
		--profile "$(RELEASE_PROFILE)"

	$(OBJCOPY) \
		-O binary \
		"$(RELEASE_ELF)" \
		"$(KERNEL_IMG)"

build-img-jtag:
	$(CARGO) build \
		--target "$(TARGET)" \
		--profile "$(JTAG_PROFILE)" \
		--features jtag

	$(OBJCOPY) \
		-O binary \
		"$(JTAG_ELF)" \
		"$(KERNEL_IMG)"

# =============================================================================
# QEMU dummy eMMC
# =============================================================================

$(QEMU_EMMC_IMG):
	@echo "[INFO] Creating sparse QEMU eMMC image: $@ ($(QEMU_EMMC_SIZE))"
	truncate -s "$(QEMU_EMMC_SIZE)" "$@"

# =============================================================================
# QEMU execution
#
# Both targets use the same QEMU command. The JTAG target only changes the
# build variant and adds QEMU's GDB server arguments.
# =============================================================================

run-qemu: build-img $(QEMU_EMMC_IMG)
	$(QEMU) $(QEMU_ARGS)

run-qemu-jtag: build-img-jtag $(QEMU_EMMC_IMG)
	$(QEMU) $(QEMU_ARGS) -s -S

# =============================================================================
# Normal deployment
# =============================================================================

deploy: build-img
	$(PYTHON) "$(KEYWORK_TOOL)" firmware "$(KERNEL_IMG)" --reboot

deploy-jtag: build-img-jtag
	$(PYTHON) "$(KEYWORK_TOOL)" firmware "$(KERNEL_IMG)" --reboot

# =============================================================================
# Cleanup
# =============================================================================

clean:
	$(CARGO) clean
	rm -f "$(KERNEL_IMG)"

# =============================================================================
# OpenOCD / GDB
# =============================================================================

openocd:
	$(OPENOCD)

gdb:
	RUST_GDB="$(RUST_GDB)" exec rust-gdb \
		-x "$(CURDIR)/.gdbinit"

gdbgui:
	RUSTC_SYSROOT="$$(rustc --print sysroot)"
	GDB_PYTHON_MODULE_DIRECTORY="$$RUSTC_SYSROOT/lib/rustlib/etc"
	RUSTC_COMMIT_HASH="$$(rustc -vV | awk '/commit-hash:/ { print $$2 }')"

	PROJECT_GDBINIT="$(CURDIR)/.gdbinit"
	GDB_COMMAND_FILE="$$(mktemp -t keywork-gdbfrontend.XXXXXX.gdb)"

	cleanup() {
		rm -f "$$GDB_COMMAND_FILE"
	}

	trap cleanup EXIT INT TERM

	printf '%s\n' \
		"directory $$GDB_PYTHON_MODULE_DIRECTORY" \
		"add-auto-load-safe-path $$GDB_PYTHON_MODULE_DIRECTORY" \
		"add-auto-load-safe-path $$PROJECT_GDBINIT" \
		"set substitute-path /rustc/$$RUSTC_COMMIT_HASH $$RUSTC_SYSROOT/lib/rustlib/src/rust" \
		"source $$PROJECT_GDBINIT" \
		>"$$GDB_COMMAND_FILE"

	export PYTHONPATH="$${PYTHONPATH:+$$PYTHONPATH:}$$GDB_PYTHON_MODULE_DIRECTORY"

	# Remove only a previous Keywork gdbfrontend session.
	$(TMUX_EXE) kill-session \
		-t "$(GDBGUI_TERMINAL_ID)" \
		2>/dev/null || true

	gdbfrontend \
		-V \
		-H "$(GDBGUI_HOST)" \
		-l "$(GDBGUI_LISTEN)" \
		-p "$(GDBGUI_PORT)" \
		-g "$(RUST_GDB)" \
		-G "-x $$GDB_COMMAND_FILE" \
		-tmux "$(TMUX_EXE)" \
		-t "$(GDBGUI_TERMINAL_ID)" \
		-w "$(CURDIR)"

# =============================================================================
# Legacy / recovery deployment through rpiboot
#
# These targets are retained only for cases where the normal Keywork firmware
# updater cannot be used.
# =============================================================================

deploy-rpiboot: build-img
deploy-rpiboot-jtag: build-img-jtag

deploy-rpiboot deploy-rpiboot-jtag:
	RPIBOOT_PID=""
	MOUNTED_DEVICE=""

	cleanup() {
		if [[ -n "$$MOUNTED_DEVICE" ]]; then
			echo "[INFO] Unmounting $$MOUNTED_DEVICE..."

			$(UDISKSCTL) unmount \
				-b "$$MOUNTED_DEVICE" \
				>/dev/null 2>&1 || true
		fi

		if [[ -n "$$RPIBOOT_PID" ]] \
			&& kill -0 "$$RPIBOOT_PID" 2>/dev/null
		then
			echo "[INFO] Stopping rpiboot..."

			kill "$$RPIBOOT_PID" 2>/dev/null || true
			wait "$$RPIBOOT_PID" 2>/dev/null || true
		fi
	}

	trap cleanup EXIT INT TERM

	echo "[WARNING] Using legacy rpiboot deployment."

	if pgrep -x rpiboot >/dev/null; then
		echo "[INFO] rpiboot is already running."
	else
		echo "[INFO] Starting rpiboot..."

		$(RPIBOOT) >"$(RPIBOOT_LOG)" 2>&1 &
		RPIBOOT_PID="$$!"
	fi

	echo "[INFO] Waiting for partition labelled '$(CM4_BOOT_LABEL)'..."

	DEVICE_PATH=""

	for ((attempt = 1; attempt <= $(DEPLOY_TIMEOUT); attempt++)); do
		DEVICE_PATH="$$( \
			lsblk -rno PATH,LABEL 2>/dev/null \
			| awk -v label="$(CM4_BOOT_LABEL)" \
				'$$2 == label { print $$1; exit }' \
		)"

		if [[ -n "$$DEVICE_PATH" ]]; then
			break
		fi

		sleep 1
	done

	if [[ -z "$$DEVICE_PATH" ]]; then
		echo "[ERROR] No partition labelled '$(CM4_BOOT_LABEL)' appeared."
		echo "[ERROR] rpiboot log: $(RPIBOOT_LOG)"

		if [[ -f "$(RPIBOOT_LOG)" ]]; then
			cat "$(RPIBOOT_LOG)"
		fi

		exit 1
	fi

	echo "[INFO] Found $$DEVICE_PATH"
	echo "[INFO] Mounting..."

	$(UDISKSCTL) mount -b "$$DEVICE_PATH"
	MOUNTED_DEVICE="$$DEVICE_PATH"

	MOUNT_POINT="$$(findmnt -n -o TARGET -S "$$DEVICE_PATH")"

	if [[ -z "$$MOUNT_POINT" ]]; then
		echo "[ERROR] Could not determine the mount point."
		exit 1
	fi

	if [[ ! -f "$(KERNEL_IMG)" ]]; then
		echo "[ERROR] $(KERNEL_IMG) does not exist."
		exit 1
	fi

	echo "[INFO] Copying $(KERNEL_IMG) to $$MOUNT_POINT/kernel8.img"

	cp -v \
		"$(KERNEL_IMG)" \
		"$$MOUNT_POINT/kernel8.img"

	sync

	echo "[INFO] Unmounting $$DEVICE_PATH..."

	$(UDISKSCTL) unmount -b "$$DEVICE_PATH"
	MOUNTED_DEVICE=""

	echo "[SUCCESS] Legacy rpiboot deployment completed."

# =============================================================================
# Serial console
# =============================================================================

serial:
	clear
	stty -F "$(SERIAL_PORT)" "$(SERIAL_BAUD)" raw -echo
	cat "$(SERIAL_PORT)"

# =============================================================================
# Help
# =============================================================================

help:
	printf '%s\n' \
		"Keywork targets:" \
		"" \
		"Build:" \
		"  make build-img" \
		"      Build the release kernel as kernel8.img." \
		"" \
		"  make build-img-jtag" \
		"      Build the JTAG-enabled kernel as kernel8.img." \
		"" \
		"QEMU:" \
		"  make run-qemu" \
		"      Build and run the release kernel." \
		"" \
		"  make run-qemu-jtag" \
		"      Build the JTAG kernel and run QEMU with -s -S." \
		"" \
		"Deployment:" \
		"  make deploy" \
		"      Build, upload and reboot through tools/keywork.py." \
		"" \
		"  make deploy-jtag" \
		"      Build the JTAG variant, upload and reboot." \
		"" \
		"Legacy deployment:" \
		"  make deploy-rpiboot" \
		"      Deploy through rpiboot and the bootfs partition." \
		"" \
		"  make deploy-rpiboot-jtag" \
		"      Deploy the JTAG build through rpiboot." \
		"" \
		"Debugging:" \
		"  make openocd" \
		"  make gdb" \
		"  make gdbgui" \
		"" \
		"Cleanup:" \
		"  make clean" \
		"Serial:" \
		"  make serial" \
		"      Configure and display SERIAL_PORT, default /dev/ttyUSB0." \
		"" \