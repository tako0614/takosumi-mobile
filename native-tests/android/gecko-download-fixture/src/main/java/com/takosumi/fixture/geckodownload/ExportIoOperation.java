package com.takosumi.fixture.geckodownload;

import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;

/** Closed, response-local failure metadata. No new I/O, clock or raw payload. */
final class ExportIoOperation {
    private enum Operation {
        UNKNOWN("unknown"), OPEN_OUTPUT("open_output"), VALIDATE("validate"),
        READ("read"), WRITE("write"), RESOURCE_CLOSE("resource_close"),
        FINAL_ORIGINAL_CLOSE("final_original_close");
        final String label;
        Operation(String label) { this.label = label; }
    }

    @FunctionalInterface interface OutputOpener { OutputStream open() throws IOException; }
    private Operation current = Operation.UNKNOWN;
    private Operation failed = Operation.UNKNOWN;
    private String bodyKind = "unavailable";
    private String origin = "unavailable";

    OutputStream openOutput(OutputOpener opener) throws IOException {
        current = Operation.OPEN_OUTPUT;
        return opener.open();
    }

    int read(InputStream input, byte[] buffer) throws IOException {
        current = Operation.READ;
        return input.read(buffer);
    }

    void write(OutputStream output, byte[] buffer, int offset, int count) throws IOException {
        current = Operation.WRITE;
        output.write(buffer, offset, count);
    }

    void beforeValidation() { current = Operation.VALIDATE; }
    // TWR supplies its own primary/suppressed exception order. This does not
    // distinguish which resource close failed or replace a preceding failure.
    void beforeResourceClose() { current = Operation.RESOURCE_CLOSE; }

    void closeOriginalFinally(InputStream input) throws IOException {
        current = Operation.FINAL_ORIGINAL_CLOSE;
        input.close();
    }

    String error(Exception e, long bytes) {
        return e.getClass().getSimpleName() + ": " + e.getMessage() + " after " + bytes + "B";
    }

    void observeFailure(Exception e, InputStream body) {
        failed = current;
        bodyKind = "unavailable";
        origin = "unavailable";
        try {
            bodyKind = projectBodyKind(body == null ? null : body.getClass().getName());
            origin = classifyException(current.label, bodyKind, e);
        } catch (Throwable unavailable) {
            // Optional metadata never replaces the original error or cleanup.
            origin = "unavailable";
        }
    }

    static String projectBodyKind(String className) {
        return className == null ? "none" : className.equals("org.mozilla.geckoview.GeckoInputStream")
            ? "gecko_input" : "other";
    }

    static String classifyException(String operation, String bodyKind, Exception e) {
        // Exact IOException has no application override of getStackTrace.
        // Other exception types remain unclassified; no raw name is kept.
        if (e == null) return "unavailable";
        if (!"read".equals(operation) || !"gecko_input".equals(bodyKind) || e.getClass() != IOException.class) return "unclassified";
        try {
            StackTraceElement[] frames = e.getStackTrace();
            return projectOrigin(operation, bodyKind, true, frames == null || frames.length == 0 ? null : frames[0]);
        } catch (Throwable unavailable) {
            return "unavailable";
        }
    }

    static String projectOrigin(String operation, String bodyKind, boolean exactIOException, StackTraceElement top) {
        if (!"read".equals(operation) || !"gecko_input".equals(bodyKind) || !exactIOException) return "unclassified";
        if (top == null) return "unavailable";
        return top.getClassName().equals("org.mozilla.geckoview.GeckoInputStream") && top.getMethodName().equals("read")
            ? "gecko_input_read" : "unclassified";
    }

    String failedOperation() { return failed.label; }
    String bodyKind() { return bodyKind; }
    String origin() { return origin; }
}
