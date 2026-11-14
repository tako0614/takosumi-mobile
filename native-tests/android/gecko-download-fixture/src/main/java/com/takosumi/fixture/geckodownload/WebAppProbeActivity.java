package com.takosumi.fixture.geckodownload;

import android.app.Activity;
import android.app.AlertDialog;
import android.content.Intent;
import android.database.Cursor;
import android.net.Uri;
import android.os.Bundle;
import android.provider.OpenableColumns;
import android.widget.Button;
import android.widget.LinearLayout;
import java.io.*;
import java.util.*;
import java.util.concurrent.*;
import org.mozilla.geckoview.*;

/** Anonymous Photopea operation comparison, not the product browser adapter.
 * Reuses Gecko's original response and file prompt. No page bridge, substitute
 * response, cookie export or second HTTP client. OS pickers own both file choices. */
public final class WebAppProbeActivity extends Activity {
    static final String ORIGIN = "https://www.photopea.com/";
    static final long MAX_EXPORT_BYTES = 2 * 1024 * 1024;
    final String work = UUID.randomUUID().toString();
    final String personal = UUID.randomUUID().toString();
    final Map<String, Page> pages = new HashMap<>();
    final ExecutorService writer = Executors.newSingleThreadExecutor();
    final List<Export> exports = new CopyOnWriteArrayList<>();
    final BlockingQueue<Export> responses = new LinkedBlockingQueue<>();
    final String exportName = "SG-export-" + UUID.randomUUID() + ".png";
    volatile String failure;
    volatile int filePrompts;
    volatile int fileSelections;
    volatile Uri selectedInput;
    volatile String inputContext;
    GeckoView view;
    Page selected;
    PendingFile pendingFile;
    Export pendingExport;
    AlertDialog saveDialog;
    boolean destroyed;
    int nextRequest = 1000;
    private final ArrayDeque<String> events = new ArrayDeque<>();

    synchronized void trace(String event) {
        if (events.size() == 32) events.removeFirst();
        events.addLast(event);
    }

    synchronized String diagnostics() { return "Web-app events: " + events + "\nFailure: " + failure; }

    static boolean site(String uri) { return uri != null && uri.startsWith(ORIGIN); }
    static boolean blob(String uri) { return uri != null && uri.startsWith("blob:" + ORIGIN); }

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        // Gecko permits one runtime per process. Each fixture still owns fresh
        // context IDs; the earlier loopback test owns none of these sessions.
        if (ResponseProbeActivity.runtime == null) {
            ResponseProbeActivity.runtime = GeckoRuntime.create(getApplicationContext(),
                new GeckoRuntimeSettings.Builder().remoteDebuggingEnabled(false).build());
        }
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        LinearLayout controls = new LinearLayout(this);
        button(controls, "Work", () -> select(work));
        button(controls, "Personal", () -> select(personal));
        layout.addView(controls);
        view = new GeckoView(this);
        layout.addView(view, new LinearLayout.LayoutParams(-1, 0, 1));
        setContentView(layout);
        select(work);
    }

    private void button(LinearLayout parent, String title, Runnable action) {
        Button button = new Button(this);
        button.setText(title);
        button.setAllCaps(false);
        button.setOnClickListener(v -> action.run());
        parent.addView(button, new LinearLayout.LayoutParams(0, -2, 1));
    }

    final class Page {
        final String context;
        final GeckoSession session;
        final PageBaseline baseline;
        volatile int generation;
        volatile String uri;
        Page(String context) {
            this.context = context;
            session = new GeckoSession(new GeckoSessionSettings.Builder().contextId(context).build());
            baseline = new PageBaseline(session, ORIGIN);
            session.setNavigationDelegate(new GeckoSession.NavigationDelegate() {
                @Override public GeckoResult<AllowOrDeny> onLoadRequest(GeckoSession owner, LoadRequest request) {
                    boolean allowed = site(request.uri) || blob(request.uri);
                    trace("navigation " + (blob(request.uri) ? "site-blob" : site(request.uri) ? "site" : "outside") + " allow=" + allowed);
                    return GeckoResult.fromValue(allowed ? AllowOrDeny.ALLOW : AllowOrDeny.DENY);
                }
                @Override public void onLocationChange(GeckoSession owner, String url,
                        List<GeckoSession.PermissionDelegate.ContentPermission> permissions, Boolean gesture) { uri = url; }
                @Override public GeckoResult<GeckoSession> onNewSession(GeckoSession owner, String uri) { return null; }
                @Override public GeckoResult<String> onLoadError(GeckoSession owner, String uri, WebRequestError error) {
                    failure = "Public page load error " + error.category + ":" + error.code;
                    return null;
                }
            });
            session.setProgressDelegate(new GeckoSession.ProgressDelegate() {
                @Override public void onPageStart(GeckoSession owner, String uri) {
                    generation++;
                    baseline.started(owner, generation);
                    if (pendingFile != null && pendingFile.page == Page.this) dismissFile();
                    if (pendingExport != null && pendingExport.page == Page.this) cancelPendingExport();
                }
                // A black WebView with an empty a11y tree cannot tell apart
                // "document still loading" from "loaded but no DOM"; record
                // the real page-stop so the next failure names which one.
                @Override public void onPageStop(GeckoSession owner, boolean success) {
                    baseline.stopped(owner, generation, success);
                    trace("page stop success=" + success + " uri=" + uri + " generation=" + generation);
                }
            });
            session.setPromptDelegate(new GeckoSession.PromptDelegate() {
                @Override public GeckoResult<PromptResponse> onFilePrompt(GeckoSession owner, FilePrompt prompt) {
                    // Bounded anonymous-fixture metadata only. Distinguish a
                    // page tap that never requests a file from a native refusal.
                    trace("file prompt selected=" + (selected == Page.this) + " destroyed=" + destroyed +
                        " site=" + site(uri) + " pendingFile=" + (pendingFile != null) +
                        " pendingExport=" + (pendingExport != null) + " type=" + prompt.type +
                        " capture=" + prompt.capture + " generation=" + generation);
                    if (selected != Page.this || destroyed || !site(uri) || pendingFile != null || pendingExport != null ||
                            prompt.type == FilePrompt.Type.FOLDER || prompt.capture != FilePrompt.Capture.NONE) {
                        return GeckoResult.fromValue(prompt.dismiss());
                    }
                    PendingFile pending = new PendingFile(Page.this, prompt, nextRequest++);
                    pendingFile = pending;
                    filePrompts++;
                    prompt.setDelegate(new PromptInstanceDelegate() {
                        @Override public void onPromptDismiss(BasePrompt dismissed) {
                            if (pendingFile == pending) dismissFile();
                        }
                    });
                    Intent pick = new Intent(Intent.ACTION_OPEN_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE).setType("image/png");
                    // This single-image experiment does not implement folder,
                    // camera or multi-file selection. The page gets one OS URI.
                    try {
                        startActivityForResult(pick, pending.code);
                        trace("file picker requested generation=" + pending.generation);
                    }
                    catch (RuntimeException e) { failure = "OS Open picker unavailable"; dismissFile(); }
                    return pending.result;
                }
            });
            session.setContentDelegate(new GeckoSession.ContentDelegate() {
                @Override public void onExternalResponse(GeckoSession owner, WebResponse response) {
                    Export transfer = new Export(Page.this, response, nextRequest++);
                    exports.add(transfer);
                    trace("original response Work=" + context.equals(work) + " blob=" + blob(response.uri));
                    if (selected != Page.this || destroyed || pendingExport != null || pendingFile != null || !site(uri) ||
                            !(site(response.uri) || blob(response.uri))) {
                        transfer.cancelUnstarted();
                        failure = "Original response has no current operation owner";
                        return;
                    }
                    pendingExport = transfer;
                    responses.add(transfer);
                    saveDialog = new AlertDialog.Builder(WebAppProbeActivity.this)
                        .setTitle("Save original response")
                        .setMessage("Photopea export; choose a new owned PNG in Downloads")
                        .setNegativeButton("Cancel", (dialog, which) -> cancelPendingExport())
                        .setPositiveButton("Choose location", (dialog, which) -> {
                            saveDialog = null;
                            if (!transfer.current()) { cancelPendingExport(); return; }
                            Intent save = new Intent(Intent.ACTION_CREATE_DOCUMENT).addCategory(Intent.CATEGORY_OPENABLE)
                                .setType("image/png").putExtra(Intent.EXTRA_TITLE, exportName);
                            try { startActivityForResult(save, transfer.code); }
                            catch (RuntimeException e) { failure = "OS Save picker unavailable"; cancelPendingExport(); }
                        }).setOnCancelListener(dialog -> cancelPendingExport()).show();
                }
                @Override public void onCrash(GeckoSession owner) { failure = "Gecko content process crashed"; }
                @Override public void onKill(GeckoSession owner) { failure = "Gecko content process killed"; }
            });
            session.open(ResponseProbeActivity.runtime);
        }
    }

    private void select(String context) {
        dismissFile();
        cancelPendingExport();
        if (selected != null) view.releaseSession();
        boolean fresh = !pages.containsKey(context);
        selected = pages.computeIfAbsent(context, Page::new);
        view.setSession(selected.session);
        if (fresh) selected.session.loadUri(ORIGIN);
        view.requestFocus();
    }

    final class PendingFile {
        final Page page;
        final int generation;
        final int code;
        final GeckoSession.PromptDelegate.FilePrompt prompt;
        final GeckoResult<GeckoSession.PromptDelegate.PromptResponse> result = new GeckoResult<>();
        PendingFile(Page page, GeckoSession.PromptDelegate.FilePrompt prompt, int code) {
            this.page = page; generation = page.generation; this.prompt = prompt; this.code = code;
        }
    }

    private void dismissFile() {
        PendingFile pending = pendingFile;
        pendingFile = null;
        if (pending == null) return;
        trace("file prompt dismissed generation=" + pending.generation);
        finishActivity(pending.code);
        if (!pending.prompt.isComplete()) pending.result.complete(pending.prompt.dismiss());
    }

    private void cancelPendingExport() {
        if (saveDialog != null) { saveDialog.dismiss(); saveDialog = null; }
        Export pending = pendingExport;
        pendingExport = null;
        if (pending != null) {
            finishActivity(pending.code);
            if (pending.finished.getCount() == 0) return;
            pending.cancelled = true;
            if (!pending.started) pending.cancelUnstarted();
        }
    }

    @Override protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        Uri uri = resultCode == RESULT_OK && data != null ? data.getData() : null;
        PendingFile pending = pendingFile;
        if (pending != null && pending.code == requestCode) {
            pendingFile = null;
            boolean current = !destroyed && selected == pending.page && pending.generation == pending.page.generation &&
                !pending.prompt.isComplete() && site(pending.page.uri);
            trace("file picker returned ok=" + (resultCode == RESULT_OK) + " current=" + current +
                " contentUri=" + (uri != null && "content".equals(uri.getScheme())));
            if (current && uri != null && "content".equals(uri.getScheme())) {
                selectedInput = uri;
                inputContext = pending.page.context;
                fileSelections++;
                Uri delivered = materialize(uri);
                trace("file picker materialized name=" + (delivered != null ? delivered.getLastPathSegment() : null));
                if (!pending.prompt.isComplete()) {
                    pending.result.complete(delivered != null
                        ? pending.prompt.confirm(getApplicationContext(), delivered)
                        : pending.prompt.dismiss());
                }
            } else if (!pending.prompt.isComplete()) pending.result.complete(pending.prompt.dismiss());
            return;
        }
        Export transfer = pendingExport;
        if (transfer == null || transfer.code != requestCode) return;
        if (!transfer.current() || uri == null || !"content".equals(uri.getScheme()) || uri.equals(selectedInput)) {
            cancelPendingExport();
            return;
        }
        try (Cursor cursor = getContentResolver().query(uri, new String[] {OpenableColumns.DISPLAY_NAME, OpenableColumns.SIZE}, null, null, null)) {
            if (cursor == null || !cursor.moveToFirst() || !exportName.equals(cursor.getString(0)) ||
                    (!cursor.isNull(1) && cursor.getLong(1) != 0)) throw new IOException("Save did not return the fresh owned document");
        } catch (Exception e) {
            failure = "OS export setup failed: " + e.getClass().getSimpleName();
            cancelPendingExport();
            return;
        }
        // The provider cursor must close before the response worker starts.
        transfer.output = uri;
        transfer.started = true;
        try { writer.execute(transfer); }
        catch (RejectedExecutionException e) {
            transfer.started = false;
            failure = "Original response writer rejected ownership";
            cancelPendingExport();
        }
    }

    final class Export implements Runnable {
        final Page page;
        final int generation;
        final int code;
        final WebResponse response;
        final CountDownLatch finished = new CountDownLatch(1);
        volatile Uri output;
        volatile long bytes;
        volatile boolean cancelled;
        volatile boolean started;
        volatile boolean closed;
        volatile String error;
        Export(Page page, WebResponse response, int code) {
            this.page = page; generation = page.generation; this.response = response; this.code = code;
            response.setReadTimeoutMillis(1000);
        }
        boolean current() { return !destroyed && selected == page && page.generation == generation; }
        void cancelUnstarted() {
            cancelled = true;
            if (finished.getCount() == 0) return;
            try { if (response.body != null) { response.body.close(); closed = true; } }
            catch (IOException e) { error = "Original stream close failed"; }
            finally { finished.countDown(); }
        }
        @Override public void run() {
            try (InputStream input = response.body;
                 OutputStream out = getContentResolver().openOutputStream(output, "w")) {
                if (input == null || out == null) throw new IOException("Missing original body or OS output");
                if (response.statusCode != 200 && !(blob(response.uri) && response.statusCode == 0)) throw new IOException("Unexpected response status");
                byte[] buffer = new byte[8192];
                while (!cancelled) {
                    int count = input.read(buffer);
                    if (count < 0) break;
                    if (bytes + count > MAX_EXPORT_BYTES) throw new IOException("Export exceeds fixture bound");
                    out.write(buffer, 0, count);
                    bytes += count;
                }
            } catch (IOException | RuntimeException e) {
                // Carry the stall position: run 43173821786 hit "Timed out"
                // inside this copy loop once, and whether bytes was 0 or
                // mid-export decides between a never-started stream and a
                // contended mid-copy gap without widening the 1 s bound.
                error = e.getClass().getSimpleName() + ": " + e.getMessage() + " after " + bytes + "B";
            }
            finally {
                // Verify original closure independently of output close failure.
                try { if (response.body != null) { response.body.close(); closed = true; } }
                catch (IOException e) { error = "Original stream close failed"; }
                finished.countDown();
            }
        }
    }

    @Override public void onDestroy() {
        destroyed = true;
        dismissFile();
        cancelPendingExport();
        if (view != null) view.releaseSession();
        selected = null;
        for (Page page : pages.values()) page.session.close();
        writer.shutdown();
        // Instrumentation waits for this writer before removing only its
        // returned OS documents. Never delete a provider file under a live writer.
        super.onDestroy();
    }

    // Gecko's file-prompt confirm resolves each answer to a filesystem path
    // and silently drops SAF document URIs it cannot map (Downloads and most
    // other providers only expose externalstorage paths). Read the real OS
    // bytes through the granted content URI and hand the page an app-owned
    // file whose name preserves the original DISPLAY_NAME for the DOM File.
    private Uri materialize(Uri uri) {
        String name = null;
        try (Cursor cursor = getContentResolver().query(uri,
                new String[] {OpenableColumns.DISPLAY_NAME}, null, null, null)) {
            if (cursor != null && cursor.moveToFirst() && !cursor.isNull(0)) name = cursor.getString(0);
        } catch (RuntimeException unreadable) { /* name falls back below */ }
        if (name == null || name.isEmpty()) name = uri.getLastPathSegment();
        if (name == null || name.isEmpty()) return null;
        File dir = new File(getCacheDir(), "inputs");
        File out = null;
        try {
            if (!dir.isDirectory() && !dir.mkdirs()) return null;
            out = new File(dir, name);
            try (InputStream in = getContentResolver().openInputStream(uri)) {
                if (in == null) return null;
                try (FileOutputStream stream = new FileOutputStream(out)) {
                    byte[] buffer = new byte[8192];
                    for (int count = in.read(buffer); count >= 0; count = in.read(buffer)) {
                        stream.write(buffer, 0, count);
                    }
                }
            }
            return Uri.fromFile(out);
        } catch (IOException | RuntimeException e) {
            if (out != null) out.delete();
            return null;
        }
    }
}
