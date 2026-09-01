/*
**
**  frontend diagnostics bridge
**
**  everything the UI learns about the user's network - tile hosts that
**  never answer, a geocoder that 403s, uncaught exceptions - used to
**  reach console.warn only, and the webview console is unreachable in
**  a release build. that is why "the map is blank" reports arrive with
**  a screenshot and nothing in the log file users are asked to attach.
**
**  this routes those messages through the gui_log command into the
**  same log target the backend writes to
**  (%LOCALAPPDATA%/com.louisdev.arnis/logs on Windows).
**
**  loaded in both the top window and the map iframe. the iframe cannot
**  rely on the Tauri global being injected into sub-frames, so it
**  forwards to the parent, which owns the single bridge to Rust.
**
*/
(function () {
    'use strict';

    var isTopFrame = window.parent === window;

    // A stuck tile host can fire hundreds of errors a second. Callers already
    // summarise; this is the backstop so a bug can never fill the user's disk.
    var BUDGET_PER_WINDOW = 40;
    var WINDOW_MS = 60000;
    var spent = 0;
    var windowStart = 0;
    var dropped = 0;

    // Guards against an exception inside the logger re-entering via the
    // window 'error' handler, which would loop.
    var delivering = false;

    function normalizeLevel(level) {
        return (level === 'warn' || level === 'error') ? level : 'info';
    }

    function deliver(level, message) {
        if (delivering) return;
        delivering = true;
        try {
            if (isTopFrame) {
                var core = window.__TAURI__ && window.__TAURI__.core;
                var invoke = core && core.invoke;
                if (typeof invoke === 'function') {
                    var result = invoke('gui_log', { level: level, message: message });
                    // Never let a rejected IPC surface as an unhandled rejection,
                    // which would come straight back through this same path.
                    if (result && typeof result.catch === 'function') result.catch(function () { });
                }
            } else {
                window.parent.postMessage({ type: 'arnisLog', level: level, message: message }, '*');
            }
        } catch (e) {
            // Logging must never break the caller.
        } finally {
            delivering = false;
        }
    }

    function withinBudget() {
        var now = Date.now();
        if (now - windowStart > WINDOW_MS) {
            windowStart = now;
            spent = 0;
            var missed = dropped;
            dropped = 0;
            if (missed > 0) {
                spent++;
                deliver('warn', '[log] suppressed ' + missed + ' further message(s) in the previous minute');
            }
        }
        if (spent < BUDGET_PER_WINDOW) {
            spent++;
            return true;
        }
        dropped++;
        return false;
    }

    /**
     * Records a diagnostic line in the application log file.
     * @param {'info'|'warn'|'error'} level
     * @param {*} message
     */
    window.arnisLog = function (level, message) {
        level = normalizeLevel(level);
        var text;
        try {
            text = String(message);
        } catch (e) {
            text = '[unstringifiable message]';
        }

        // Mirror to the console so a dev build still shows it inline.
        var sink = level === 'error' ? console.error : (level === 'warn' ? console.warn : console.log);
        try {
            sink.call(console, text);
        } catch (e) { }

        if (withinBudget()) deliver(level, text);
    };

    // Identity check rather than an origin string: it is scheme-independent
    // (the app is served from tauri://, where origins are awkward) and strictly
    // stronger, since only a frame this window actually embeds can match.
    function isOwnChildFrame(source) {
        if (!source) return false;
        for (var i = 0; i < window.frames.length; i++) {
            if (window.frames[i] === source) return true;
        }
        return false;
    }

    // The map iframe posts here; the top frame is the only one that can invoke.
    if (isTopFrame) {
        window.addEventListener('message', function (event) {
            var data = event.data;
            if (!data || data.type !== 'arnisLog') return;
            // Anything can postMessage to this window. Only relay our own
            // frames, so nothing else can write into the application log.
            if (!isOwnChildFrame(event.source)) return;
            var text;
            try {
                text = String(data.message);
            } catch (e) {
                return;
            }
            if (withinBudget()) deliver(normalizeLevel(data.level), '[map] ' + text);
        });
    }

    window.addEventListener('error', function (event) {
        var where = (event.filename || '?') + ':' + (event.lineno || 0);
        window.arnisLog('error', 'Uncaught error: ' + (event.message || 'unknown') + ' at ' + where);
    });

    window.addEventListener('unhandledrejection', function (event) {
        var reason = event.reason;
        var text;
        try {
            text = (reason && (reason.stack || reason.message)) || String(reason);
        } catch (e) {
            text = 'unknown';
        }
        window.arnisLog('error', 'Unhandled promise rejection: ' + text);
    });

    // One line identifying the webview, so a bug report says which engine and
    // platform produced the tile failures below it.
    if (isTopFrame) {
        window.arnisLog('info', 'UI started (' + navigator.userAgent + ')');
    }
})();
