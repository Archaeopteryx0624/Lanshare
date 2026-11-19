package dev.archaeopteryx.lanshare;

import android.Manifest;
import android.app.Activity;
import android.content.Intent;
import android.net.Uri;
import android.os.Build;
import android.os.Bundle;
import android.os.Environment;
import android.provider.Settings;
import android.webkit.WebResourceError;
import android.webkit.WebResourceRequest;
import android.webkit.WebView;
import android.webkit.WebViewClient;

public class MainActivity extends Activity {
    private static final String URL = "http://127.0.0.1:" + FileServer.PORT + "/";
    private WebView web;
    private boolean asked;

    @Override protected void onCreate(Bundle b) {
        super.onCreate(b);
        if (Build.VERSION.SDK_INT < 30) {
            requestPermissions(new String[]{Manifest.permission.READ_EXTERNAL_STORAGE,
                    Manifest.permission.WRITE_EXTERNAL_STORAGE}, 1);
        }
        Intent si = new Intent(this, ShareService.class);
        if (Build.VERSION.SDK_INT >= 26) startForegroundService(si); else startService(si);

        web = new WebView(this);
        web.getSettings().setJavaScriptEnabled(true);
        web.setWebViewClient(new WebViewClient() {
            @Override public void onReceivedError(WebView v, WebResourceRequest r, WebResourceError e) {
                // Server may still be starting: retry shortly.
                if (r.isForMainFrame()) v.postDelayed(() -> v.loadUrl(URL), 700);
            }
        });
        setContentView(web);
        web.loadUrl(URL);
    }

    @Override protected void onResume() {
        super.onResume();
        if (Build.VERSION.SDK_INT >= 30 && !Environment.isExternalStorageManager() && !asked) {
            asked = true;
            startActivity(new Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION,
                    Uri.parse("package:" + getPackageName())));
        }
    }

    @Override public void onBackPressed() { moveTaskToBack(true); }
}
