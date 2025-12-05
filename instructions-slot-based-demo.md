# INSTRUCTIONS TO RUN THE SLOT BASED DEMO

1. compile with fast runtime. 
cargo +1.88.0-x86_64-unknown-linux-gnu build  --features=fast-runtime --release

2. download zombienet from https://github.com/paritytech/zombienet and make it executable

3. spawn zombienet with the config called test-slots-based.toml, in the root of the directory:
e.g., for linux users: ./zombienet-linux-x64_latest spawn test-slot-based.toml -p native

4. Wait until the 1st session to happen on the relay-chain to start seeing blocks in glutton.

5. Modify the glutton target rate (up to 3 for now). For that called sudo.setStorage and set this item:
- key: 0x774fc20a0bfea123a0b9b914c507fe04
- value: 0x02000000 for 3 seconds blocks or 0x03000000 for 2 seconds blocks

And that's it! enjoy!