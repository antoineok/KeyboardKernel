# Load your ELF file (with symbols)
file target/aarch64-unknown-none/debug/kernel

# boot.gdb – automatically attach, unlock, and start firmware

# Connect to the target
# (adjust port if you use OpenOCD, probe-rs, or J-Link)
target extended-remote localhost:3333

b breakpoint

# Show where the flag lives (for verification)
info address DEBUGGER_FLAG

# Set the debugger flag to 1 to wake firmware
set DEBUGGER_FLAG = 1

# Confirm it changed (optional)
x/w &DEBUGGER_FLAG
