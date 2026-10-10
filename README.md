# expyandex-rs

since our beloved yandex is facing performance issues lately here is a load test to help us monitor performance.

# methodology

a couple of relevant api have been identified that will help put a safe load on the services.

# build

```
cargo build --release
```

# invoke

windows:

```
target\release\expyandex-rs.exe
```

linux, macos:

```
./target/release/expyandex-rs
```

## parameters

`-n` is how many of each test, json and image, run together. `0` runs until ctrl+c. default is `0`.

`unpack` writes `json-1mb.json` and `image.png` next to the executable. a normal run does that itself when either file is missing.
