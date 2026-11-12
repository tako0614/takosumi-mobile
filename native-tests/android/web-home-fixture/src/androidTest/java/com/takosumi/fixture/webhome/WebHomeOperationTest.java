package com.takosumi.fixture.webhome;

import static org.junit.Assert.*;

import android.content.Context;
import android.os.Bundle;
import android.os.SystemClock;
import android.provider.Settings;
import androidx.test.ext.junit.runners.AndroidJUnit4;
import androidx.test.platform.app.InstrumentationRegistry;
import androidx.test.uiautomator.By;
import androidx.test.uiautomator.BySelector;
import androidx.test.uiautomator.UiDevice;
import androidx.test.uiautomator.UiObject2;
import java.io.File;
import java.io.FileOutputStream;
import java.nio.charset.StandardCharsets;
import java.util.ArrayList;
import java.util.List;
import java.util.UUID;
import java.util.regex.Pattern;
import org.json.JSONArray;
import org.json.JSONObject;
import org.junit.After;
import org.junit.Before;
import org.junit.Test;
import org.junit.runner.RunWith;

/** External real-UI operations only: no WebView, JS injection or product callbacks. */
@RunWith(AndroidJUnit4.class)
public final class WebHomeOperationTest {
    private static final String TARGET = "com.takosumi.fixture.android";
    private static final String DRIVER = "com.takosumi.fixture.webhome";
    private UiDevice device;
    private Context context;
    private String origin, editor, launcher;
    private final JSONObject result = new JSONObject();
    private final JSONArray cases = new JSONArray();

    @Before public void setup() throws Exception {
        context = InstrumentationRegistry.getInstrumentation().getTargetContext();
        assertEquals("The driver must not replace the product APK", DRIVER, context.getPackageName());
        Bundle args = InstrumentationRegistry.getArguments();
        origin = args.getString("instance");
        editor = args.getString("editor");
        launcher = args.getString("launcher");
        assertNotNull(origin);
        assertNotNull(editor);
        assertNotNull(launcher);
        assertTrue(origin.matches("https://127\\.0\\.0\\.1:[0-9]{1,5}"));
        assertTrue(editor.matches("https://localhost:[0-9]{1,5}"));
        assertTrue(launcher.matches(Pattern.quote(TARGET) + "/" + Pattern.quote(TARGET) + "\\.[A-Za-z0-9_.$]+"));
        result.put("passed", false).put("cases", cases);
        device = UiDevice.getInstance(InstrumentationRegistry.getInstrumentation());
        launch();
        one(By.text("保存して開く"));
    }

    private UiObject2 one(BySelector selector) {
        BySelector inApp = selector.pkg(TARGET);
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            UiObject2 value = device.findObject(inApp);
            if (value != null && value.isEnabled() && !value.getVisibleBounds().isEmpty()) return value;
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Actual App control did not become ready: " + selector);
    }

    private void expect(String text) { one(By.text(text)); }
    private void click(String text) { one(By.text(text)).click(); }
    private void nativeButton(String description) { one(By.desc(description)).click(); }
    private void menu(String text) { nativeButton("メニュー"); click(text); }
    private void picker() { menu("接続先を切り替える"); expect("接続先を追加"); }

    private void hideKeyboardIfShown() {
        String ime = Settings.Secure.getString(context.getContentResolver(), Settings.Secure.DEFAULT_INPUT_METHOD);
        if (ime != null && ime.contains("/")) {
            String owner = ime.substring(0, ime.indexOf('/'));
            if (device.hasObject(By.pkg(owner))) device.pressBack();
        }
    }

    private void add(String name) {
        expect("保存して開く");
        List<UiObject2> fields = device.findObjects(By.clazz("android.widget.EditText").pkg(TARGET));
        assertEquals("The real connection form must contain URL and name", 2, fields.size());
        fields.get(0).click(); fields.get(0).setText(origin);
        fields.get(1).click(); fields.get(1).setText(name);
        hideKeyboardIfShown();
        click("保存して開く");
        expect("Fixture signed out");
        expect("Remote has no App IPC");
    }

    private void reopen(String name) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            List<UiObject2> matches = new ArrayList<>();
            for (UiObject2 label : device.findObjects(By.text(name).pkg(TARGET))) {
                UiObject2 row = label;
                for (int depth = 0; row != null && depth < 4; depth++, row = row.getParent()) {
                    if ("android.widget.Button".equals(row.getClassName()) && row.isEnabled()
                            && (String.valueOf(row.getText()).contains(origin)
                                || String.valueOf(row.getContentDescription()).contains(origin)
                                || row.findObject(By.textContains(origin)) != null)) {
                        matches.add(row); break;
                    }
                }
            }
            if (matches.size() == 1) { matches.get(0).click(); return; }
            assertTrue("Ambiguous saved connection " + name, matches.size() <= 1);
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Saved App connection missing: " + name);
    }

    private void host(String address) {
        one(By.desc("表示中のサイト " + address.substring("https://".length())));
    }

    private void draft(String value) {
        long end = SystemClock.elapsedRealtime() + 15000;
        do {
            List<UiObject2> fields = device.findObjects(By.clazz("android.widget.EditText").pkg(TARGET));
            if (fields.size() == 1 && value.equals(fields.get(0).getText())) {
                expect("Remote has no App IPC"); return;
            }
            SystemClock.sleep(100);
        } while (SystemClock.elapsedRealtime() < end);
        throw new AssertionError("Wrong profile's actual editor draft: " + value);
    }

    private void writeDraft(String value) {
        draft("");
        UiObject2 field = one(By.clazz("android.widget.EditText"));
        field.click(); field.setText(value);
        hideKeyboardIfShown();
        draft(value);
    }

    private String document() {
        String text = one(By.text(Pattern.compile("Document [0-9a-fA-F-]{36}"))).getText();
        UUID.fromString(text.substring("Document ".length()));
        return text;
    }

    private String pid() throws Exception {
        String value = device.executeShellCommand("pidof " + TARGET).trim();
        assertTrue("Ambiguous App process", value.isEmpty() || value.matches("[1-9][0-9]*"));
        return value;
    }

    private void launch() throws Exception {
        String reply = device.executeShellCommand("am start -W -n " + launcher);
        assertTrue("Actual App launcher refused", reply.contains("Status: ok"));
    }

    private void closeAndOpenMain() throws Exception {
        nativeButton("閉じる");
        // Android removes this profile's Recents task. The ordinary App icon
        // may be used to return to the connection picker, independent of the
        // platform's choice of the next foreground Recents task.
        launch(); expect("接続先を追加");
    }

    @Test public void wholeAppConnectionsWebHomeAndResumeStayProfileBound() throws Exception {
        add("Work"); host(origin);
        click("Sign in Work"); expect("Fixture home Work"); expect("Instance data instance-Work");
        click("Account settings"); expect("Account settings Work");
        nativeButton("戻る"); expect("Fixture home Work");
        click("📝 Notes"); expect("Notes editor"); host(editor);
        writeDraft("work-draft");
        click("Page two"); expect("Notes page two");
        nativeButton("戻る"); expect("Notes editor"); draft("work-draft");
        menu("アプリホームへ"); expect("Fixture home Work");
        nativeButton("戻る"); draft("work-draft");
        cases.put("connection-web-login-home-back").put("cross-origin-no-ipc");
        String warm = document(), warmPid = pid();
        assertFalse(warmPid.isEmpty());
        picker(); reopen("Work"); draft("work-draft"); assertEquals(warm, document());
        device.pressHome(); launch(); expect("接続先を追加"); reopen("Work");
        draft("work-draft"); assertEquals(warmPid, pid()); assertEquals(warm, document());
        cases.put("warm-document-retained");
        picker(); add("Personal"); click("Sign in Personal"); expect("Fixture home Personal");
        click("📝 Notes"); writeDraft("personal-draft");
        picker(); reopen("Work"); draft("work-draft");
        menu("アプリホームへ"); expect("Fixture home Work");
        nativeButton("戻る"); draft("work-draft");
        cases.put("profile-auth-and-draft-isolated");
        String before = document(), oldPid = pid();
        picker();
        device.executeShellCommand("am force-stop " + TARGET);
        long end = SystemClock.elapsedRealtime() + 10000;
        while (!pid().isEmpty() && SystemClock.elapsedRealtime() < end) SystemClock.sleep(100);
        assertEquals("Original App must terminate", "", pid());
        launch(); expect("接続先を追加"); reopen("Work");
        expect("Notes editor"); draft("work-draft"); host(editor);
        String fresh = document(), freshPid = pid();
        assertFalse(freshPid.isEmpty()); assertNotEquals(oldPid, freshPid); assertNotEquals(before, fresh);
        result.put("oldPid", oldPid).put("newPid", freshPid).put("beforeColdDocument", before).put("afterColdDocument", fresh);
        menu("アプリホームへ"); expect("Fixture home Work");
        picker(); reopen("Personal"); draft("personal-draft");
        menu("アプリホームへ"); expect("Fixture home Personal");
        cases.put("cold-process-and-profiles-restored");
        closeAndOpenMain(); reopen("Work"); closeAndOpenMain();
        click("保存データを消去…"); expect("Workの保存データを消去しますか？");
        click("保存データを消去"); expect("Workのサイトデータを消去しました。");
        reopen("Work"); expect("Fixture signed out"); host(origin);
        click("Sign in Work"); expect("Fixture home Work"); click("📝 Notes"); draft("");
        closeAndOpenMain(); reopen("Personal");
        expect("Fixture home Personal"); expect("Instance data instance-Personal");
        click("📝 Notes"); draft("personal-draft"); closeAndOpenMain();
        cases.put("cleared-profile-reset").put("other-profile-preserved").put("native-views-closed");
        result.put("passed", true);
    }

    @After public void evidence() throws Exception {
        File files = context == null ? null : context.getFilesDir();
        if (files == null) return;
        boolean saved = false;
        try {
            if (device != null) {
                boolean screenshot = device.takeScreenshot(new File(files, "screen.png"));
                device.dumpWindowHierarchy(new File(files, "hierarchy.xml"));
                saved = screenshot;
            }
        } catch (Exception error) {
            result.put("diagnosticError", error.getClass().getSimpleName());
        }
        result.put("diagnosticsSaved", saved);
        try (FileOutputStream stream = new FileOutputStream(new File(files, "result.json"))) {
            stream.write(result.toString().getBytes(StandardCharsets.UTF_8));
        }
    }
}
