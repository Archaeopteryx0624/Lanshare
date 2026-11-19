package dev.archaeopteryx.lanshare;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.Service;
import android.content.Intent;
import android.content.SharedPreferences;
import android.content.pm.ServiceInfo;
import android.os.Build;
import android.os.Environment;
import android.os.IBinder;
import fi.iki.elonen.NanoHTTPD;
import java.io.IOException;
import java.util.Random;

/** Keeps the file server alive while the app is in the background. */
public class ShareService extends Service {
    private static FileServer server;

    @Override public int onStartCommand(Intent intent, int flags, int startId) {
        String ch = "share";
        if (Build.VERSION.SDK_INT >= 26) {
            getSystemService(NotificationManager.class).createNotificationChannel(
                    new NotificationChannel(ch, "File sharing", NotificationManager.IMPORTANCE_LOW));
        }
        Notification.Builder nb = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(this, ch) : new Notification.Builder(this);
        Notification n = nb.setContentTitle("LAN Share is running")
                .setContentText("Other devices can connect on port " + FileServer.PORT)
                .setSmallIcon(android.R.drawable.stat_sys_upload).build();
        if (Build.VERSION.SDK_INT >= 29) {
            startForeground(1, n, ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC);
        } else {
            startForeground(1, n);
        }

        if (server == null) {
            SharedPreferences sp = getSharedPreferences("lanshare", MODE_PRIVATE);
            String pin = sp.getString("pin", null);
            if (pin == null) {
                pin = String.valueOf(100000 + new Random().nextInt(900000));
                sp.edit().putString("pin", pin).apply();
            }
            try {
                server = new FileServer(getApplicationContext(), Build.MODEL, pin,
                        Environment.getExternalStorageDirectory());
                server.start(NanoHTTPD.SOCKET_READ_TIMEOUT, false);
            } catch (IOException e) {
                server = null;
            }
        }
        return START_STICKY;
    }

    @Override public IBinder onBind(Intent i) { return null; }
}
