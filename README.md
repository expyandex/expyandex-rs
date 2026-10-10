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
