# HiyoDyna

[English](README.md) | **Tiếng Việt**

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

Một overlay kiểu Dynamic Island viết bằng Rust native cho Windows. Bản hiện tại cung cấp capsule đồng hồ trực tiếp, mở rộng khi rê chuột; sẽ bổ sung thêm nhiều tính năng hệ thống.

## Tính năng hiện có

- Capsule mở rộng khi rê chuột và thu lại sau một khoảng ngắn với hiệu ứng lò xo
- Đồng hồ trực tiếp với định dạng 24 giờ hoặc 12 giờ, giây nhỏ hơn và chỉ báo màu xanh
- Bảng mở rộng hiển thị thứ và ngày phía trên giờ lớn
- Win32 native + render phần mềm — không dùng webview, HTML hay JavaScript
- Biểu tượng khay hệ thống và menu chuột phải cho các preset Motion, Bounce, Shadow, Position và Clock
- Cài đặt lưu tại `%APPDATA%\HiyoDyna\settings.ini`
- Mặc định sát mép trên, căn giữa, không xuất hiện trên taskbar và không kích hoạt khi nhấp

## Yêu cầu

- Windows 10 hoặc 11
- Rust stable hỗ trợ edition 2024

## Chạy

```powershell
cargo run --release
```

Rê chuột lên capsule để mở bảng đồng hồ; di chuyển chuột ra ngoài, bảng thu lại sau một khoảng ngắn. Nhấp chuột trái không bật/tắt bảng. Nhấp chuột phải lên capsule hoặc biểu tượng khay để mở các preset Motion, Bounce, Shadow, Position và Clock; chọn Exit trong menu để thoát.

Preset Position có thể thêm khoảng cách 8 hoặc 16 px dưới mép trên. Đồng hồ mặc định dùng 24 giờ và có thể chuyển sang 12 giờ trong menu.

## Build

```powershell
cargo build --release
```

Tệp thực thi được ghi vào `target\release\hiyodyna.exe`.

## Phát hành

Tải bản dựng từ [trang Releases](https://github.com/hiyo41/HiyoDyna/releases). Mỗi file zip gồm executable, hai bản README và giấy phép.

Bản pre-release là bản thử nghiệm từ `main`. Bản chính thức ổn định và có changelog đầy đủ.

Gặp lỗi? [Mở issue](https://github.com/hiyo41/HiyoDyna/issues).

## Phát triển

```powershell
.\scripts\check.ps1
```

CI chạy các kiểm tra tương tự trên `windows-latest`.

## Đóng góp

Mọi đóng góp đều được hoan nghênh. Xem [CONTRIBUTING.md](CONTRIBUTING.md).

## Giấy phép

GNU Affero General Public License v3.0 — xem [LICENSE](LICENSE).
