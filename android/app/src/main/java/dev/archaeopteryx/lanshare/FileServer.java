package dev.archaeopteryx.lanshare;

import android.content.Context;
import fi.iki.elonen.NanoHTTPD;
import java.io.*;
import java.net.*;
import java.util.*;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicLong;
import org.json.JSONArray;
import org.json.JSONObject;

/**
 * Same API as the Windows app.
 *  /api/*  peer API, needs header x-token = this device's PIN
 *  /ui/*   used by the local web UI, loopback only
 */
public class FileServer extends NanoHTTPD {
    public static final int PORT = 8765;
    private final Context ctx;
    private final String name, pin;
    private final File root;
    private final AtomicLong done = new AtomicLong(), total = new AtomicLong();
    /** each: {id, name, ip, port, pin} */
    private final List<String[]> peers = new CopyOnWriteArrayList<>();

    public FileServer(Context ctx, String name, String pin, File root) {
        super(PORT);
        this.ctx = ctx; this.name = name; this.pin = pin; this.root = root;
    }

    // ---------- routing ----------
    @Override public Response serve(IHTTPSession s) {
        try {
            String uri = s.getUri();
            if (uri.equals("/")) return newFixedLengthResponse(Response.Status.OK, "text/html", asset("index.html"));
            if (uri.equals("/api/info")) return json(new JSONObject().put("name", name).put("platform", "android"));
            if (uri.startsWith("/api/")) {
                if (!pin.equals(s.getHeaders().get("x-token"))) return fail(Response.Status.UNAUTHORIZED, "wrong pin");
                return api(uri, s);
            }
            if (uri.startsWith("/ui/")) {
                String ip = s.getRemoteIpAddress();
                if (!"127.0.0.1".equals(ip) && !"0:0:0:0:0:0:0:1".equals(ip)) return fail(Response.Status.FORBIDDEN, "local only");
                return ui(uri, s);
            }
            return fail(Response.Status.NOT_FOUND, "not found");
        } catch (IOException e) {
            return fail(Response.Status.BAD_REQUEST, String.valueOf(e.getMessage()));
        } catch (Exception e) {
            return fail(Response.Status.INTERNAL_ERROR, String.valueOf(e));
        }
    }

    private Response api(String uri, IHTTPSession s) throws Exception {
        switch (uri) {
            case "/api/list":
                return json(list(p(s, "path")));
            case "/api/download": {
                File f = resolve(p(s, "path"));
                if (f == null || !f.isFile()) return fail(Response.Status.NOT_FOUND, "no such file");
                return newFixedLengthResponse(Response.Status.OK, "application/octet-stream", new FileInputStream(f), f.length());
            }
            case "/api/upload": {
                File dir = resolve(p(s, "path"));
                String n = new File(p(s, "name")).getName();
                if (dir == null || !dir.isDirectory() || n.isEmpty()) return fail(Response.Status.BAD_REQUEST, "bad path");
                File dest = unique(new File(dir, n));
                long len = Long.parseLong(s.getHeaders().getOrDefault("content-length", "0"));
                try (OutputStream o = new FileOutputStream(dest)) { copy(s.getInputStream(), o, len); }
                return json(new JSONObject().put("saved", dest.getName()));
            }
            case "/api/mkdir": {
                File d = resolve(p(s, "path"));
                if (d == null || (!d.isDirectory() && !d.mkdirs())) return fail(Response.Status.BAD_REQUEST, "cannot create folder");
                return fail(Response.Status.OK, "ok");
            }
            case "/api/hello": {
                JSONObject b = body(s);
                addPeer(b.getString("name"), s.getRemoteIpAddress(), b.getInt("port"), b.getString("pin"));
                return fail(Response.Status.OK, "ok");
            }
        }
        return fail(Response.Status.NOT_FOUND, "not found");
    }

    private Response ui(String uri, IHTTPSession s) throws Exception {
        switch (uri) {
            case "/ui/state": {
                JSONArray ps = new JSONArray();
                for (String[] p : peers) ps.put(new JSONObject().put("id", p[0]).put("name", p[1]).put("ip", p[2]));
                return json(new JSONObject().put("name", name).put("pin", pin).put("ip", localIp())
                        .put("port", PORT).put("peers", ps));
            }
            case "/ui/list":
                return json(list(p(s, "path")));
            case "/ui/progress":
                return json(new JSONObject().put("done", done.get()).put("total", total.get()));
            case "/ui/mkdir": {
                File d = resolve(body(s).getString("path"));
                if (d == null || (!d.isDirectory() && !d.mkdirs())) return fail(Response.Status.BAD_REQUEST, "Cannot create folder");
                return fail(Response.Status.OK, "ok");
            }
            case "/ui/rmkdir": {
                JSONObject b = body(s);
                String[] pr = peer(b.getString("peer"));
                if (pr == null) return fail(Response.Status.BAD_REQUEST, "Unknown device");
                post(url(pr, "/api/mkdir") + "?path=" + enc(b.getString("path")), pr[4], "");
                return fail(Response.Status.OK, "ok");
            }
            case "/ui/rlist": {
                String[] pr = peer(p(s, "peer"));
                if (pr == null) return fail(Response.Status.BAD_REQUEST, "Unknown device");
                try {
                    String t = get(url(pr, "/api/list") + "?path=" + enc(p(s, "path")), pr[4]);
                    return newFixedLengthResponse(Response.Status.OK, "application/json", t);
                } catch (IOException e) {
                    return fail(Response.Status.BAD_REQUEST, "Cannot reach that device. Is it still running and on the same wifi?");
                }
            }
            case "/ui/connect": {
                JSONObject b = body(s);
                String host = b.getString("ip").trim(), ip = host, pinIn = b.getString("pin");
                int port = PORT;
                if (host.contains(":")) {
                    ip = host.substring(0, host.indexOf(':'));
                    try { port = Integer.parseInt(host.substring(host.indexOf(':') + 1)); } catch (Exception ignored) {}
                }
                String base = "http://" + ip + ":" + port;
                try {
                    get(base + "/api/list?path=", pinIn);
                } catch (IOException e) {
                    if ("Wrong PIN".equals(e.getMessage())) throw e;
                    throw new IOException("Cannot reach that device. Check the IP and that both are on the same wifi.");
                }
                String pn = ip;
                try { pn = new JSONObject(get(base + "/api/info", null)).getString("name"); } catch (Exception ignored) {}
                try {
                    post(base + "/api/hello", pinIn, new JSONObject().put("name", name).put("port", PORT).put("pin", pin).toString());
                } catch (IOException ignored) {}
                String[] np = addPeer(pn, ip, port, pinIn);
                return json(new JSONObject().put("id", np[0]).put("name", np[1]).put("ip", np[2]));
            }
            case "/ui/pull": {
                JSONObject b = body(s);
                String[] pr = peer(b.getString("peer"));
                File dir = resolve(b.getString("localDir"));
                if (pr == null) return fail(Response.Status.BAD_REQUEST, "Unknown device");
                if (dir == null || !dir.isDirectory()) return fail(Response.Status.BAD_REQUEST, "Bad folder");
                String rp = b.getString("remotePath");
                HttpURLConnection c = open(url(pr, "/api/download") + "?path=" + enc(rp), pr[4]);
                check(c);
                File dest = unique(new File(dir, new File(rp).getName()));
                total.set(Math.max(0, c.getContentLengthLong()));
                done.set(0);
                try (InputStream in = c.getInputStream(); OutputStream o = new FileOutputStream(dest)) { copy(in, o, -1, done); }
                return json(new JSONObject().put("saved", dest.getName()));
            }
            case "/ui/push": {
                JSONObject b = body(s);
                String[] pr = peer(b.getString("peer"));
                File src = resolve(b.getString("localPath"));
                if (pr == null) return fail(Response.Status.BAD_REQUEST, "Unknown device");
                if (src == null || !src.isFile()) return fail(Response.Status.BAD_REQUEST, "Bad file");
                HttpURLConnection c = open(url(pr, "/api/upload") + "?path=" + enc(b.getString("remoteDir"))
                        + "&name=" + enc(src.getName()), pr[4]);
                c.setRequestMethod("PUT");
                c.setDoOutput(true);
                c.setReadTimeout(60000);
                c.setFixedLengthStreamingMode(src.length());
                total.set(src.length());
                done.set(0);
                try (OutputStream o = c.getOutputStream(); InputStream in = new FileInputStream(src)) { copy(in, o, -1, done); }
                check(c);
                return json(new JSONObject().put("sent", src.getName()));
            }
        }
        return fail(Response.Status.NOT_FOUND, "not found");
    }

    // ---------- files ----------
    private File resolve(String rel) {
        File f = root;
        for (String part : rel.split("[/\\\\]")) {
            if (part.isEmpty() || part.equals(".")) continue;
            if (part.equals("..")) return null;
            f = new File(f, part);
        }
        return f;
    }

    private JSONObject list(String rel) throws Exception {
        File d = resolve(rel);
        if (d == null || !d.isDirectory()) throw new IOException("bad path");
        File[] fs = d.listFiles();
        if (fs == null) fs = new File[0];
        Arrays.sort(fs, (a, b) -> a.isDirectory() != b.isDirectory()
                ? (a.isDirectory() ? -1 : 1) : a.getName().compareToIgnoreCase(b.getName()));
        JSONArray arr = new JSONArray();
        for (File f : fs) {
            arr.put(new JSONObject().put("name", f.getName()).put("dir", f.isDirectory())
                    .put("size", f.length()).put("mtime", f.lastModified() / 1000));
        }
        return new JSONObject().put("path", rel).put("entries", arr);
    }

    private static File unique(File f) {
        if (!f.exists()) return f;
        String n = f.getName(), stem = n, ext = "";
        int dot = n.lastIndexOf('.');
        if (dot > 0) { stem = n.substring(0, dot); ext = n.substring(dot); }
        for (int i = 1; ; i++) {
            File c = new File(f.getParentFile(), stem + " (" + i + ")" + ext);
            if (!c.exists()) return c;
        }
    }

    // ---------- peers & http ----------
    private String[] addPeer(String pname, String ip, int port, String ppin) {
        String id = ip + ":" + port;
        for (String[] p : peers) if (p[0].equals(id)) peers.remove(p);
        String[] np = {id, pname, ip, String.valueOf(port), ppin};
        peers.add(np);
        return np;
    }
    private String[] peer(String id) {
        for (String[] p : peers) if (p[0].equals(id)) return p;
        return null;
    }
    private static String url(String[] p, String path) { return "http://" + p[2] + ":" + p[3] + path; }

    private static HttpURLConnection open(String u, String pin) throws IOException {
        HttpURLConnection c = (HttpURLConnection) new URL(u).openConnection();
        c.setConnectTimeout(5000);
        c.setReadTimeout(30000);
        if (pin != null) c.setRequestProperty("x-token", pin);
        return c;
    }
    private static void check(HttpURLConnection c) throws IOException {
        int code = c.getResponseCode();
        if (code == 401) throw new IOException("Wrong PIN");
        if (code >= 400) throw new IOException("Device answered " + code);
    }
    private static String get(String u, String pin) throws IOException {
        HttpURLConnection c = open(u, pin);
        check(c);
        try (InputStream in = c.getInputStream()) {
            ByteArrayOutputStream o = new ByteArrayOutputStream();
            copy(in, o, -1);
            return o.toString("UTF-8");
        }
    }
    private static void post(String u, String pin, String body) throws IOException {
        HttpURLConnection c = open(u, pin);
        c.setRequestMethod("POST");
        c.setDoOutput(true);
        c.setRequestProperty("content-type", "application/json");
        try (OutputStream o = c.getOutputStream()) { o.write(body.getBytes("UTF-8")); }
        check(c);
    }

    // ---------- small utils ----------
    /** limit < 0 copies until EOF */
    private static void copy(InputStream in, OutputStream out, long limit) throws IOException {
        copy(in, out, limit, null);
    }
    private static void copy(InputStream in, OutputStream out, long limit, AtomicLong prog) throws IOException {
        byte[] buf = new byte[65536];
        long left = limit;
        while (left != 0) {
            int want = left < 0 ? buf.length : (int) Math.min(buf.length, left);
            int n = in.read(buf, 0, want);
            if (n < 0) break;
            out.write(buf, 0, n);
            if (prog != null) prog.addAndGet(n);
            if (left > 0) left -= n;
        }
    }
    private JSONObject body(IHTTPSession s) throws Exception {
        int len = Integer.parseInt(s.getHeaders().getOrDefault("content-length", "0"));
        byte[] b = new byte[len];
        new DataInputStream(s.getInputStream()).readFully(b);
        return new JSONObject(new String(b, "UTF-8"));
    }
    private static String p(IHTTPSession s, String k) {
        String v = s.getParms().get(k);
        return v == null ? "" : v;
    }
    private static String enc(String x) throws IOException { return URLEncoder.encode(x, "UTF-8"); }
    private String asset(String n) throws IOException {
        try (InputStream in = ctx.getAssets().open(n)) {
            ByteArrayOutputStream o = new ByteArrayOutputStream();
            copy(in, o, -1);
            return o.toString("UTF-8");
        }
    }
    private static Response json(Object o) {
        return newFixedLengthResponse(Response.Status.OK, "application/json", o.toString());
    }
    private static Response fail(Response.IStatus st, String m) {
        return newFixedLengthResponse(st, "text/plain", m);
    }
    private static String localIp() {
        try {
            for (NetworkInterface ni : Collections.list(NetworkInterface.getNetworkInterfaces())) {
                if (!ni.isUp() || ni.isLoopback()) continue;
                for (InetAddress a : Collections.list(ni.getInetAddresses())) {
                    if (a instanceof Inet4Address && a.isSiteLocalAddress()) return a.getHostAddress();
                }
            }
        } catch (Exception ignored) {}
        return "";
    }
}
