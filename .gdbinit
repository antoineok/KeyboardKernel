set pagination off
set confirm off
set target-async on

# Load your ELF file (with symbols)
file target/aarch64-unknown-none/gdb_release/kernel


python
import gdb, re

class SetDebuggerFlag(gdb.Command):
    def __init__(self):
        super().__init__("set_debugger_flag", gdb.COMMAND_USER)

    def invoke(self, arg, from_tty):
        args = gdb.string_to_argv(arg)
        value = args[0] if args else "1"
        should_continue = args[1].lower() == "continue" or "c" if len(args) > 1 else False

        out = gdb.execute("info address DEBUGGER_FLAG", to_string=True)
        m = re.search(r'0x[0-9a-fA-F]+', out)
        if not m:
            raise gdb.GdbError("DEBUGGER_FLAG not found")

        addr = m.group(0)
        print(f"DEBUGGER_FLAG found at {addr}")

        gdb.execute(f"set *({addr} as *const u32) = {value}")
        print(f"DEBUGGER_FLAG set to {value} at {addr}")
        gdb.execute(f"x/w {addr}")

        if should_continue:
            gdb.execute("c")

SetDebuggerFlag()
end

# Connect to the target
# (adjust port if you use OpenOCD, probe-rs, or J-Link)
target extended-remote localhost:3333

b breakpoint
hbreak kernel::panic::panic

# Show where the flag lives (for verification)
info address DEBUGGER_FLAG

# Set the debugger flag to 1 to wake firmware
set_debugger_flag 1