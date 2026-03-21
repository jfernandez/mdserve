# מדריך להתקנה ותצורה

## מבוא

מסמך זה מתאר את תהליך ההתקנה והתצורה של mdserve, שרת תצוגה מקדימה של Markdown המעוצב להתחברות עם סוכני קוד בינה מלאכותית.

## דרישות מקדימות

- Rust 1.82 ומעלה
- מערכת הפעלה תומכת (macOS, Linux, Windows)
- שורת פקודה או terminal

## התקנה

### שיטה 1: בנייה מהקוד

```bash
git clone https://github.com/anthropic/mdserve.git
cd mdserve
cargo build --release
./target/release/mdserve /path/to/file.md
```

### שיטה 2: התקנה עם Cargo

```bash
cargo install --path .
mdserve /path/to/file.md
```

## שימוש בסיסי

### צפייה בקובץ יחיד

```bash
mdserve document.md
```

### צפייה בתיקייה

```bash
mdserve /path/to/docs/
```

## אפשרויות שורת פקודה

| אפשרות | תיאור |
|--------|-------|
| `-H, --hostname` | כתובת IP או שם מארח (ברירת מחדל: 127.0.0.1) |
| `-p, --port` | פורט להאזנה (ברירת מחדל: 3000) |
| `-o, --open` | פתח בדפדפן המערכת |
| `--rtl` | אלץ RTL לכל המסמכים |
| `--no-rtl` | בטל גילוי RTL אוטומטי |

## תכונות

- **גילוי אוטומטי של RTL**: מזהה עברית, ערבית, פרסית ושפות RTL נוספות
- **עריכה בזמן אמת**: טעינה מחדש אוטומטית כאשר הקבצים משתנים
- **תמות**: תיאור, אור, Catppuccin Latte/Macchiato/Mocha
- **תמיכה ב-Mermaid**: תרשימים ותרשימי זרימה אינטראקטיביים
- **אפס תצורה**: פשוט הפעל את mdserve עם קובץ או תיקייה

## דוגמאות

> mdserve משתמש בגישת "אפס תצורה" - אין קבצי תצורה להגדרה. פשוט הפעל את הפקודה ותמצא את הממשק בדפדפן שלך.

### דוגמה עם RTL כפוי

```bash
mdserve --rtl hebrew_document.md
```

### דוגמה עם LTR כפוי

```bash
mdserve --no-rtl document.md
```

## שגיאות נפוצות

- **שגיאה**: "No markdown files found in directory"
  - **פתרון**: ודא שהתיקייה מכילה קבצי `.md` או `.markdown`

- **שגיאה**: "Port already in use"
  - **פתרון**: mdserve יחפש פורט פנוי באופן אוטומטי

## תמיכה

לשאלות ובעיות, בקר ב-[GitHub Issues](https://github.com/anthropic/mdserve/issues)
