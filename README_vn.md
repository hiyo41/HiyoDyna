# HiyoDyna

[English](README.md) | **Tiếng Việt**

[![CI](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml/badge.svg)](https://github.com/hiyo41/HiyoDyna/actions/workflows/ci.yml)

Một overlay kiểu Dynamic Island viết bằng Rust native cho Windows. Bản hiện tại cung cấp capsule đồng hồ trực tiếp, mở rộng khi rê chuột; sẽ bổ sung thêm nhiều tính năng hệ thống.

## Tính năng hiện có

- Capsule mở rộng khi rê chuột và thu lại sau một khoảng ngắn với hiệu ứng lò xo
- Đồng hồ trực tiếp với định dạng 24 giờ hoặc 12 giờ, giây nhỏ hơn và chỉ báo màu nhấn
- Typography Inter đi kèm, góc squircle, bóng mềm và quầng sáng khi rê chuột
- Bảng mở rộng hiển thị thứ và ngày phía trên giờ lớn
- Win32 native + render phần mềm — không dùng webview, HTML hay JavaScript
- Biểu tượng khay hệ thống và menu chuột phải cho Motion, Bounce, Shadow, Accent, Seconds, Position và Clock
- Tùy chọn khởi động cùng Windows, cùng các mục khởi động lại, mở thư mục cài đặt và giới thiệu
- Cài đặt lưu tại `%APPDATA%\HiyoDyna\settings.ini`
- Mặc định sát mép trên, căn giữa, không xuất hiện trên taskbar và không kích hoạt khi nhấp

## Yêu cầu

- Windows 10 hoặc 11
- Rust stable hỗ trợ edition 2024 để build và cho watcher tự cập nhật

## Cài đặt

```powershell
.\scripts\setup.ps1
```

Bản cài theo người dùng nằm ở `%LOCALAPPDATA%\Programs\HiyoDyna`. Thiết lập cũng tạo shortcut Desktop và Start Menu, mục gỡ cài đặt và watcher ẩn trong Startup. Dùng `-NoAutostart` hoặc `-NoWatcher` để bỏ qua từng phần.

Trình cài này dành cho checkout cục bộ và cần Rust để build lại. Bản tải từ Releases chạy executable đóng gói sẵn, không có watcher.

## Tự cập nhật

Watcher theo dõi `src\**`, `assets\**`, `Cargo.toml` và `Cargo.lock`. Khi các tệp này thay đổi, nó build lại bản release rồi khởi động lại app đã cài. Thay đổi thực hiện khi watcher tắt sẽ được xử lý ở lần khởi động kế tiếp.

- Log: `%LOCALAPPDATA%\HiyoDyna\auto-update.log`
- Tạm dừng: tạo `%LOCALAPPDATA%\HiyoDyna\pause`; xóa tệp để tiếp tục
- Build lỗi sẽ giữ app đang chạy và chờ lần chỉnh sửa kế tiếp

## Gỡ cài đặt

Dùng **Settings > Apps** hoặc chạy:

```powershell
%LOCALAPPDATA%\Programs\HiyoDyna\uninstall.ps1
```

Thêm `-KeepSettings` để giữ `%APPDATA%\HiyoDyna` và `%LOCALAPPDATA%\HiyoDyna`.

## Chạy

```powershell
cargo run --release
```

Rê chuột lên capsule để mở bảng đồng hồ; di chuyển chuột ra ngoài, bảng thu lại sau một khoảng ngắn. Nhấp chuột trái không bật/tắt bảng. Nhấp chuột phải lên capsule hoặc biểu tượng khay để mở các preset Motion, Bounce, Shadow, Accent, Seconds, Position và Clock; menu cũng có Restart, Open settings folder, About và Exit.

Preset Position có thể thêm khoảng cách 8 hoặc 16 px dưới mép trên. Đồng hồ mặc định dùng 24 giờ và có thể chuyển sang 12 giờ trong menu.

## Build

```powershell
cargo build --release
```

Tệp thực thi được ghi vào `target\release\hiyodyna.exe`.

## Phát hành

Tải bản dựng từ [trang Releases](https://github.com/hiyo41/HiyoDyna/releases). Mỗi file zip gồm executable, hai bản README, giấy phép và giấy phép font Inter.

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

Font Inter đi kèm theo SIL Open Font License; xem [assets/fonts/OFL.txt](assets/fonts/OFL.txt).
