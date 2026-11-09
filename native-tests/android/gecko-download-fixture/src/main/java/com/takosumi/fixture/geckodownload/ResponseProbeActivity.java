package com.takosumi.fixture.geckodownload;

import android.app.Activity;
import android.os.Bundle;
import android.widget.Button;
import android.widget.LinearLayout;
import java.io.*;
import java.nio.file.Files;
import java.util.*;
import java.util.concurrent.*;
import org.mozilla.geckoview.*;

/** Candidate capability host only; no product adapter, JS bridge, cookie export or URL refetch. */
public final class ResponseProbeActivity extends Activity {
    static GeckoRuntime runtime;
    final String work = UUID.randomUUID().toString();
    final String personal = UUID.randomUUID().toString();
    final BlockingQueue<Transfer> responses = new LinkedBlockingQueue<>();
    final List<Transfer> transfers = new CopyOnWriteArrayList<>();
    final Map<String, GeckoSession> sessions = new HashMap<>();
    final ExecutorService writer = Executors.newSingleThreadExecutor();
    FixtureServer server;
    GeckoView view;
    GeckoSession selected;
    File directory;
    volatile String failure;
    boolean destroyed;

    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        try {
            directory = Files.createTempDirectory(getCacheDir().toPath(), "gecko-response-").toFile();
            Files.write(new File(directory, "keep.txt").toPath(), new byte[] {11, 5});
            server = new FixtureServer();
            if (runtime == null) runtime = GeckoRuntime.create(getApplicationContext(), new GeckoRuntimeSettings.Builder()
                .remoteDebuggingEnabled(false).build());
            LinearLayout layout = new LinearLayout(this);
            layout.setOrientation(LinearLayout.VERTICAL);
            LinearLayout buttons = new LinearLayout(this);
            addButton(buttons, "Work", () -> select(work, false));
            addButton(buttons, "Personal", () -> select(personal, false));
            addButton(buttons, "Reopen Work", () -> select(work, true));
            layout.addView(buttons);
            LinearLayout actions = new LinearLayout(this);
            addButton(actions, "Cancel transfer", this::cancelTransfers);
            addButton(actions, "Close Work", () -> {
                cancelTransfers();
                if (selected == sessions.get(work)) { view.releaseSession(); selected = null; }
                GeckoSession old = sessions.remove(work);
                if (old != null) old.close();
            });
            layout.addView(actions);
            view = new GeckoView(this);
            layout.addView(view, new LinearLayout.LayoutParams(-1, 0, 1));
            setContentView(layout);
            select(work, false);
        } catch (Exception e) { throw new RuntimeException(e); }
    }

    private void addButton(LinearLayout row, String label, Runnable action) {
        Button button = new Button(this);
        button.setText(label);
        button.setOnClickListener(v -> action.run());
        row.addView(button, new LinearLayout.LayoutParams(0, -2, 1));
    }

    private GeckoSession create(String context) {
        GeckoSession session = new GeckoSession(new GeckoSessionSettings.Builder().contextId(context).build());
        session.setNavigationDelegate(new GeckoSession.NavigationDelegate() {
            @Override public GeckoResult<AllowOrDeny> onLoadRequest(GeckoSession owner, LoadRequest request) {
                boolean local = request.uri.startsWith(server.url()) || request.uri.startsWith("blob:" + server.url());
                return GeckoResult.fromValue(local ? AllowOrDeny.ALLOW : AllowOrDeny.DENY);
            }
        });
        session.setContentDelegate(new GeckoSession.ContentDelegate() {
            @Override public void onExternalResponse(GeckoSession owner, WebResponse response) {
                Transfer transfer = new Transfer(owner, context, response);
                transfers.add(transfer);
                if (destroyed) {
                    transfer.closeUnstarted("Owner destroyed before response delivery");
                    return;
                }
                try {
                    writer.execute(transfer);
                    responses.add(transfer);
                } catch (RejectedExecutionException e) {
                    transfer.closeUnstarted("Response executor rejected ownership");
                    failure = "Response executor rejected ownership";
                }
            }
            @Override public void onCrash(GeckoSession owner) { failure = "Gecko content process crashed"; }
        });
        session.open(runtime);
        return session;
    }

    private void select(String context, boolean reopen) {
        cancelTransfers();
        if (selected != null) view.releaseSession();
        if (reopen) {
            GeckoSession old = sessions.remove(context);
            if (old != null) old.close();
        }
        selected = sessions.computeIfAbsent(context, this::create);
        view.setSession(selected);
        selected.loadUri(server.url());
    }

    private void cancelTransfers() {
        for (Transfer transfer : transfers) if (transfer.finished.getCount() != 0) transfer.cancelled = true;
    }

    final class Transfer implements Runnable {
        final GeckoSession owner;
        final String context;
        final WebResponse response;
        final CountDownLatch finished = new CountDownLatch(1);
        volatile long bytes;
        volatile boolean cancelled;
        volatile boolean closed;
        volatile File file;
        volatile String error;

        Transfer(GeckoSession owner, String context, WebResponse response) {
            this.owner = owner; this.context = context; this.response = response;
            // Pending callbacks from an inactive session must never become a new download.
            if (owner != selected) cancelled = true;
            response.setReadTimeoutMillis(1000);
        }

        void closeUnstarted(String reason) {
            cancelled = true;
            error = reason;
            try {
                if (response.body != null) { response.body.close(); closed = true; }
            } catch (IOException e) { failure = "Undelivered original response did not close"; }
            finally { finished.countDown(); }
        }

        @Override public void run() {
            InputStream body = response.body;
            boolean outputClosed = true;
            try {
                if (body == null) throw new IOException("External response has no original body");
                boolean blob = response.uri.startsWith("blob:" + server.url());
                if (!(response.uri.startsWith(server.url()) || blob)) throw new IOException("Non-fixture response");
                if (response.statusCode != 200 && !(blob && response.statusCode == 0)) throw new IOException("Unexpected download status " + response.statusCode);
                if (!cancelled) {
                    file = File.createTempFile("owned-", ".bin", directory);
                    OutputStream out = new FileOutputStream(file);
                    outputClosed = false;
                    try {
                        byte[] buffer = new byte[8192];
                        while (!cancelled) {
                            int count = body.read(buffer);
                            if (count < 0) break;
                            if (bytes + count > FixtureServer.SLOW_BYTES) throw new IOException("Unexpected response size");
                            out.write(buffer, 0, count);
                            bytes += count;
                        }
                    } finally {
                        out.close();
                        outputClosed = true;
                    }
                }
            } catch (IOException e) {
                error = e.getClass().getSimpleName() + ": " + e.getMessage();
            } finally {
                try {
                    if (body != null) body.close();
                    closed = body != null;
                } catch (IOException e) { error = "Original response stream close failed"; }
                // Only retire the partial after both actual stream/output closes.
                if (closed && outputClosed && (cancelled || error != null) && file != null && !file.delete()) error = "Owned partial file was not deleted";
                finished.countDown();
            }
        }
    }

    @Override public void onDestroy() {
        destroyed = true;
        cancelTransfers();
        if (view != null) view.releaseSession();
        selected = null;
        for (GeckoSession session : sessions.values()) session.close();
        sessions.clear();
        writer.shutdown();
        // Do not delete a directory which may still be used by the stream owner.
        super.onDestroy();
    }
}
