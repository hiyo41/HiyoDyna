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
- Biểu tượng khay hệ thống và menu chuột phải cho Motion, Bounce, Shadow, Accent, Seconds, Position, Clock và Update
- Bộ cập nhật trong app với kênh Build và Stable, xác minh SHA-256, kiểm tra lúc khởi động và mỗi 30 phút
- Giao diện đen/xám/trắng thống nhất với màu nhấn pastel trên Island, các cửa sổ native và biểu tượng khay
- Cửa sổ Cài đặt và Tìm lời bài hát native có thể đổi kích thước, cùng chuyển đổi trực tiếp giữa tiếng Anh và tiếng Việt
- Tùy chọn khởi động cùng Windows, cùng các mục khởi động lại, mở thư mục cài đặt và giới thiệu
- Cài đặt lưu tại `%APPDATA%\HiyoDyna\settings.ini`
- Mặc định sát mép trên, căn giữa, không xuất hiện trên taskbar và không kích hoạt khi nhấp

## Yêu cầu

- Windows 10 hoặc 11
- Rust stable hỗ trợ edition 2024 khi build từ mã nguồn

## Cài đặt

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\setup.ps1
```

Bản cài theo người dùng nằm ở `%LOCALAPPDATA%\Programs\HiyoDyna`. Thiết lập tạo shortcut Desktop và Start Menu, mục gỡ cài đặt và bật khởi động cùng Windows. Dùng `-NoAutostart` để bỏ qua khởi động cùng Windows.

Trình cài này build checkout cục bộ và cần Rust. Bản tải từ Releases chạy executable đóng gói sẵn và cập nhật qua bộ cập nhật trong app.

## Tự cập nhật

HiyoDyna tự cập nhật từ GitHub. App kiểm tra lúc khởi động và mỗi 30 phút; menu **Update** trên khay cho phép đổi kênh hoặc kiểm tra ngay.

- **Build** theo bản pre-release từ `main` (`pre-N`).
- **Stable** theo bản chính thức; trước khi có bản chính thức đầu tiên, kênh này dùng pre-release mới nhất.
- Bản tải được xác minh SHA-256 trước khi đưa vào hàng chờ và áp dụng.
- Log: `%LOCALAPPDATA%\HiyoDyna\auto-update.log`

Bộ cập nhật chạy trong HiyoDyna. Chỉ một tiến trình phụ ngắn hạn được tạo khi thay executable; không có watcher thường trú và không có shortcut Startup cho việc cập nhật.

## Gỡ cài đặt

Dùng **Settings > Apps** hoặc chạy:

```powershell
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "$env:LOCALAPPDATA\Programs\HiyoDyna\uninstall.ps1"
```

Thêm `-KeepSettings` để giữ `%APPDATA%\HiyoDyna` và `%LOCALAPPDATA%\HiyoDyna`.

## Chạy

```powershell
cargo run --release
```

Rê chuột lên capsule để mở bảng đồng hồ; di chuyển chuột ra ngoài, bảng thu lại sau một khoảng ngắn. Nhấp chuột trái không bật/tắt bảng. Nhấp chuột phải lên capsule hoặc biểu tượng khay để mở các preset Motion, Bounce, Shadow, Accent, Seconds, Position, Clock và Update; menu cũng có Restart, Open settings folder, About và Exit.

Mở **Cài đặt** từ biểu tượng bánh răng trên Island mở rộng hoặc từ khay hệ thống. Cửa sổ dùng bố cục hai cột: thanh danh mục bên trái và các thẻ nội dung có thể cuộn, với phần đầu và chân cố định. Khi cửa sổ hẹp, thanh bên chuyển thành hàng danh mục ngang; chọn danh mục sẽ cuộn đến phần tương ứng. Có thể di chuyển, đổi kích thước, thu nhỏ và để cửa sổ mở khi dùng ứng dụng khác; thay đổi có hiệu lực ngay. Tab/Shift+Tab đưa từng điều khiển vào vùng nhìn thấy, Enter/Space kích hoạt, Page Up/Down cuộn và Escape đóng cửa sổ.

Capsule thu gọn hiển thị lời có thời gian và tự điều chỉnh chiều rộng; khi không có thời gian phù hợp, nó hiển thị đồng hồ. Trước câu tiếp theo, lời sắp hát có màu dịu hơn, vẫn giữ biểu tượng nốt nhạc và quầng sáng lấy từ bìa. Trong ba giây cuối, lời và nốt nhạc dao động độ sáng nhẹ khi đang phát; tạm dừng sẽ dừng hiệu ứng, còn capsule vẫn giữ nguyên. Đồng hồ đếm ngược căn theo đường cơ sở của chữ chỉ xuất hiện khi thời gian chờ dài hơn ba giây.

Chọn **Tìm lời bài hát** ở chân trình phát hoặc menu nhanh của Island để tìm bằng tên bài hát và nghệ sĩ có thể chỉnh sửa. Cửa sổ native có phần đầu hiển thị bài đang phát, thẻ tìm kiếm, các hàng kết quả có thể cuộn và chân cửa sổ cố định chứa trạng thái cùng nút thao tác. Kết quả có nhãn nhà cung cấp, thời lượng và nhãn thời gian: **Theo từ**, **Theo dòng** hoặc **Chưa rõ** trước khi tải. Các trạng thái ban đầu, đang tìm, đã chọn kết quả, trống, lỗi nhà cung cấp/mạng, không lời, không có phiên phát đích và đã xếp hàng áp dụng giúp làm rõ thao tác tiếp theo. Kết quả đã biết là không có thời gian được lọc bỏ; lời tải về phải có thời gian đồng bộ mới được áp dụng. Chọn bản ghi rồi chọn **Dùng lời bài hát đã chọn**; cửa sổ đóng ngay khi lựa chọn được xếp hàng. Lựa chọn giữ hiệu lực đến khi đổi bài hoặc làm mới lời; đổi truy vấn hoặc bài hát sẽ hủy yêu cầu cũ. Tìm kiếm chạy nền, không chặn điều khiển phát nhạc. Lựa chọn thủ công chỉ áp dụng cho phiên phát hiện tại và không được lưu vào bộ nhớ đệm ghép lời tự động.

Preset Position có thể thêm khoảng cách 8 hoặc 16 px dưới mép trên. Đồng hồ mặc định dùng 24 giờ và có thể chuyển sang 12 giờ trong menu.

## Build

```powershell
cargo build --release
```

Tệp thực thi được ghi vào `target\release\hiyodyna.exe`.

## Phát hành

Tải bản dựng từ [trang Releases](https://github.com/hiyo41/HiyoDyna/releases). Mỗi bản phát hành có một file zip gồm executable, hai bản README, giấy phép và giấy phép font Inter, cùng executable thô và SHA-256 cho bộ cập nhật trong app.

Bản pre-release là bản thử nghiệm từ `main` và cấp cho kênh Build. Bản chính thức cấp cho kênh Stable và có changelog đầy đủ.

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
