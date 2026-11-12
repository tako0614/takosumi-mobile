package com.takosumi.fixture.webhome;

import java.util.regex.Pattern;

/** The WebView's merged accessible name for a saved connection button. */
final class ConnectionLabel {
    private ConnectionLabel() { }

    static boolean matches(String label, String name, String origin) {
        if (label == null || name.isEmpty() || origin.isEmpty()) return false;
        String value = Pattern.quote(name) + "\\s+" + Pattern.quote(origin)
            + "(?:\\s+(?:開く|前の画面へ戻る)\\s+→)?";
        return Pattern.compile(value).matcher(label).matches();
    }
}
