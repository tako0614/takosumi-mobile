package com.takosumi.fixture.geckodownload;

import java.io.*;
import java.net.*;
import java.nio.charset.StandardCharsets;
import java.util.Set;
import java.util.concurrent.*;
import java.util.concurrent.atomic.AtomicInteger;

/** Real loopback HTTP transport, not a response/delegate substitution. No real account. */
final class FixtureServer implements AutoCloseable {
    static final byte[] COOKIE = "Original Gecko cookie response\n".getBytes(StandardCharsets.UTF_8);
    static final byte[] BLOB = "Original Gecko Blob response\n".getBytes(StandardCharsets.UTF_8);
    static final int SLOW_BYTES = 4 * 1024 * 1024;
    final AtomicInteger authenticatedDownloads = new AtomicInteger();
    final AtomicInteger deniedDownloads = new AtomicInteger();
    final AtomicInteger interruptedSlowResponses = new AtomicInteger();
    private final ServerSocket listener;
    private final Set<Socket> sockets = ConcurrentHashMap.newKeySet();
    private final ExecutorService workers = Executors.newCachedThreadPool();
    private volatile boolean stopped;
    private final Thread acceptor;

    FixtureServer() throws IOException {
        listener = new ServerSocket(0, 8, InetAddress.getByName("127.0.0.1"));
        acceptor = new Thread(() -> {
            while (!stopped) {
                try {
                    Socket socket = listener.accept();
                    sockets.add(socket);
                    workers.execute(() -> serve(socket));
                } catch (IOException e) {
                    if (!stopped) throw new RuntimeException(e);
                }
            }
        }, "owned-gecko-fixture-http");
        acceptor.start();
    }

    String url() { return "http://127.0.0.1:" + listener.getLocalPort() + "/"; }

    private void serve(Socket socket) {
        boolean slow = false;
        int written = 0;
        try (socket) {
            socket.setSoTimeout(5000);
            BufferedReader input = new BufferedReader(new InputStreamReader(socket.getInputStream(), StandardCharsets.US_ASCII));
            String request = input.readLine();
            if (request == null || request.length() > 4096) return;
            String[] parts = request.split(" ");
            if (parts.length != 3 || !parts[0].equals("GET")) return;
            String cookie = "";
            int size = 0;
            for (String line; (line = input.readLine()) != null && !line.isEmpty();) {
                size += line.length();
                if (size > 8192) return;
                if (line.toLowerCase(java.util.Locale.ROOT).startsWith("cookie:")) cookie = line.substring(7).trim();
            }
            boolean signedIn = java.util.Arrays.asList(cookie.split(";\\s*")).contains("fixture=work");
            OutputStream output = socket.getOutputStream();
            String path = parts[1];
            if (path.equals("/login")) {
                headers(output, 303, "text/plain", 0, "Location: /\r\nSet-Cookie: fixture=work; Path=/; HttpOnly; SameSite=Lax\r\n");
            } else if (path.equals("/cookie") || path.equals("/slow")) {
                if (!signedIn) {
                    deniedDownloads.incrementAndGet();
                    html(output, 403, "<h1>Cookie denied</h1>");
                } else if (path.equals("/cookie")) {
                    authenticatedDownloads.incrementAndGet();
                    headers(output, 200, "application/octet-stream", COOKIE.length, "Content-Disposition: attachment; filename=cookie.txt\r\n");
                    output.write(COOKIE);
                    output.flush();
                } else {
                    slow = true;
                    headers(output, 200, "application/octet-stream", SLOW_BYTES, "Content-Disposition: attachment; filename=slow.bin\r\n");
                    byte[] chunk = new byte[8192];
                    java.util.Arrays.fill(chunk, (byte) 83);
                    while (written < SLOW_BYTES) {
                        output.write(chunk);
                        output.flush();
                        written += chunk.length;
                        Thread.sleep(100);
                    }
                }
            } else if (path.equals("/")) {
                // Inline handlers resolve bare URL through document.URL before the window constructor.
                html(output, 200, "<h1>" + (signedIn ? "Fixture signed in" : "Fixture signed out") + "</h1>"
                    + "<a href='/login'>Fixture sign in</a><br><a href='/cookie'>Cookie attachment</a><br>"
                    + "<button onclick=\"this.textContent='Blob click received';try{const a=document.createElement('a');a.href=window.URL.createObjectURL(new Blob(['Original Gecko Blob response\\n'],{type:'application/octet-stream'}));a.download='blob.txt';a.click();this.textContent='Blob click dispatched';}catch(e){this.textContent='Blob click failed';}\">Blob export</button><br>"
                    + "<a href='/slow'>Slow attachment</a>");
            } else {
                html(output, 404, "Missing fixture route");
            }
        } catch (IOException e) {
            // Count before fixture teardown only, with real incomplete bytes.
            if (slow && written > 0 && written < SLOW_BYTES && !stopped) interruptedSlowResponses.incrementAndGet();
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        } finally {
            sockets.remove(socket);
        }
    }

    private static void headers(OutputStream out, int code, String type, int length, String extra) throws IOException {
        out.write(("HTTP/1.1 " + code + " Fixture\r\nContent-Type: " + type + "\r\nContent-Length: " + length
            + "\r\nCache-Control: no-store\r\nConnection: close\r\n" + extra + "\r\n").getBytes(StandardCharsets.US_ASCII));
        out.flush();
    }

    private static void html(OutputStream out, int code, String body) throws IOException {
        byte[] bytes = ("<!doctype html><meta name='viewport' content='width=device-width,initial-scale=1'>"
            + "<style>a,button{display:inline-block;font-size:20px;margin:10px}</style>" + body).getBytes(StandardCharsets.UTF_8);
        headers(out, code, "text/html; charset=utf-8", bytes.length, "");
        out.write(bytes);
    }

    @Override public void close() throws IOException {
        stopped = true;
        listener.close();
        for (Socket socket : sockets) socket.close();
        workers.shutdownNow();
        try {
            acceptor.join(3000);
            if (acceptor.isAlive() || !workers.awaitTermination(3, TimeUnit.SECONDS)) throw new IOException("Owned HTTP threads did not stop");
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
            throw new IOException(e);
        }
    }
}
