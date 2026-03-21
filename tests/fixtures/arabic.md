# دليل التثبيت والإعدادات

## المقدمة

تصف هذه الوثيقة عملية تثبيت وإعداد mdserve، وهو خادم معاينة Markdown مصمم للعمل مع وكلاء الترميز الذكي.

## المتطلبات الأساسية

- Rust 1.82 أو أحدث
- نظام تشغيل مدعوم (macOS أو Linux أو Windows)
- واجهة سطر الأوامر أو Terminal

## التثبيت

### الطريقة الأولى: البناء من المصدر

```bash
git clone https://github.com/anthropic/mdserve.git
cd mdserve
cargo build --release
./target/release/mdserve /path/to/file.md
```

### الطريقة الثانية: التثبيت باستخدام Cargo

```bash
cargo install --path .
mdserve /path/to/file.md
```

## الاستخدام الأساسي

### عرض ملف واحد

```bash
mdserve document.md
```

### عرض مجلد

```bash
mdserve /path/to/docs/
```

## خيارات سطر الأوامر

| الخيار | الوصف |
|--------|-------|
| `-H, --hostname` | عنوان IP أو اسم المضيف (الافتراضي: 127.0.0.1) |
| `-p, --port` | المنفذ للاستماع (الافتراضي: 3000) |
| `-o, --open` | فتح في متصفح النظام |
| `--rtl` | فرض RTL لجميع المستندات |
| `--no-rtl` | تعطيل الكشف التلقائي عن RTL |

## الميزات

- **الكشف التلقائي عن RTL**: يكتشف العربية والعبرية والفارسية واللغات RTL الأخرى
- **التحديث المباشر**: إعادة تحميل تلقائية عند تغيير الملفات
- **المواضيع**: فاتح وداكن و Catppuccin Latte/Macchiato/Mocha
- **دعم Mermaid**: مخططات بيانية وخرائط تدفق تفاعلية
- **بدون إعدادات**: ما عليك سوى تشغيل mdserve مع ملف أو مجلد

## أمثلة

> mdserve تستخدم نهج "بدون إعدادات" - لا توجد ملفات إعدادات للتكوين. ما عليك سوى تشغيل الأمر وستجد الواجهة في متصفحك.

### مثال مع فرض RTL

```bash
mdserve --rtl arabic_document.md
```

### مثال مع فرض LTR

```bash
mdserve --no-rtl document.md
```

## الأخطاء الشائعة

- **خطأ**: "No markdown files found in directory"
  - **الحل**: تأكد من أن المجلد يحتوي على ملفات `.md` أو `.markdown`

- **خطأ**: "Port already in use"
  - **الحل**: سيبحث mdserve تلقائياً عن منفذ مجاني

## الدعم

للأسئلة والمشاكل، يرجى زيارة [مشاكل GitHub](https://github.com/anthropic/mdserve/issues)
