package dev.zerocode.text;

import android.app.UiAutomation;
import android.os.Bundle;
import android.os.HandlerThread;
import android.os.Looper;
import android.util.Base64;
import android.view.accessibility.AccessibilityNodeInfo;
import java.lang.reflect.Constructor;
import java.lang.reflect.Method;

/**
 * Puts text at the cursor of the focused field on an Android device, through
 * accessibility, for the characters `input text` cannot type.
 *
 * <p>`input text` turns text into key presses on the virtual keyboard's
 * character map (InputShellCommand.sendText, KeyCharacterMap.getEvents), and
 * that map holds no Hangul, emoji or ₩: getEvents answers null and the command
 * dies. A key event carrying characters (ACTION_MULTIPLE) is refused by the
 * input dispatcher. So the field's own text is rewritten with the new words at
 * the selection, and the cursor put after them; nothing touches the clipboard.
 *
 * <p>Run by the window as the shell user: {@code CLASSPATH=<this dex>
 * app_process / dev.zerocode.text.Insert <base64 UTF-8 text>}. One line on
 * stdout says what happened; the exit code is 0 when the text went in, 3 when
 * no editable field has the input focus, 4 when the field refused it.
 */
public final class Insert {
    private Insert() {}

    public static void main(String[] args) throws Exception {
        String text = new String(Base64.decode(args[0], Base64.DEFAULT), "UTF-8");
        if (Looper.getMainLooper() == null) Looper.prepareMainLooper();
        HandlerThread thread = new HandlerThread("zerocode-text");
        thread.start();
        UiAutomation ui = connect(thread.getLooper());
        AccessibilityNodeInfo field = focusedField(ui);
        if (field == null) {
            System.out.println("{\"inserted\":false,\"why\":\"no_editable_focus\"}");
            System.out.flush();
            System.exit(3);
        }
        CharSequence shown = field.isShowingHintText() ? "" : field.getText();
        String now = shown == null ? "" : shown.toString();
        int start = field.getTextSelectionStart();
        int end = field.getTextSelectionEnd();
        if (start < 0 || end < 0 || start > now.length() || end > now.length()) {
            start = now.length();
            end = now.length();
        }
        int from = Math.min(start, end);
        String next = now.substring(0, from) + text + now.substring(Math.max(start, end));
        Bundle set = new Bundle();
        set.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, next);
        if (!field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, set)) {
            System.out.println("{\"inserted\":false,\"why\":\"field_refused\"}");
            System.out.flush();
            System.exit(4);
        }
        int caret = from + text.length();
        field.refresh();
        Bundle at = new Bundle();
        at.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, caret);
        at.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, caret);
        boolean placed = field.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, at);
        System.out.println("{\"inserted\":true,\"caret\":" + caret + ",\"caretPlaced\":" + placed + "}");
        System.out.flush();
        System.exit(0);
    }

    /** The shell's own UiAutomation, as `uiautomator dump` makes it. */
    private static UiAutomation connect(Looper looper) throws Exception {
        Class<?> connectionClass = Class.forName("android.app.UiAutomationConnection");
        Object connection = connectionClass.getConstructor().newInstance();
        Class<?> connectionInterface = Class.forName("android.app.IUiAutomationConnection");
        Constructor<UiAutomation> make = UiAutomation.class.getDeclaredConstructor(Looper.class, connectionInterface);
        make.setAccessible(true);
        UiAutomation ui = make.newInstance(looper, connection);
        Method connect = UiAutomation.class.getDeclaredMethod("connect");
        connect.setAccessible(true);
        connect.invoke(ui);
        return ui;
    }

    /** The field holding the input focus, if one is editable; asked again for a second while windows settle. */
    private static AccessibilityNodeInfo focusedField(UiAutomation ui) throws InterruptedException {
        for (int attempt = 0; attempt < 20; attempt++) {
            AccessibilityNodeInfo root = ui.getRootInActiveWindow();
            AccessibilityNodeInfo focus = root == null ? null : root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
            if (focus != null) return focus.isEditable() ? focus : null;
            Thread.sleep(50);
        }
        return null;
    }
}
