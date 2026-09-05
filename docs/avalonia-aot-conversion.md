# Avalonia Native AOT conversion

The investigation and first implementation live under [`dotnet/README.md`](../dotnet/README.md).

The Qt/C++ application remains the production UI. The `dotnet/` tree is a
parallel rewrite that isolates analysis from the GUI, categorizes audio/video
content, suggests zip/tar project entities, stores suggestions in an AOT-safe
SQLite database, and can round-trip remote path proposals before applying
changes locally.
