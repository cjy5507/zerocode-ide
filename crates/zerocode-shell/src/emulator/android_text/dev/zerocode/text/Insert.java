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
 * app_process / dev.zerocode.text.Insert <base64 UTF-8 text>}, or with
 * {@code --check} alone to say only whether the focus may be typed into. One
 * line on stdout says what happened; the exit code is 0 when the text went in
 * (or may), 3 when no editable field has the input focus, 4 when the field
 * refused it, 5 when the field is a password: its text reads as dots, so
 * rewriting it would lose what the person typed, and a secret is theirs to
 * type. 6 when the field kept its old text (a filter refused it), 7 when the
 * text is longer than the field takes. A field that formats what it is given
 * counts as taking it.
 */
public final class Insert {
    private Insert() {}

    public static void main(String[] args) throws Exception {
        boolean check = "--check".equals(args[0]);
        String text = check ? "" : new String(Base64.decode(args[0], Base64.DEFAULT), "UTF-8");
        if (Looper.getMainLooper() == null) Looper.prepareMainLooper();
        HandlerThread thread = new HandlerThread("zerocode-text");
        thread.start();
        UiAutomation ui = connect(thread.getLooper());
        // A check before keys is asked on every typed line: it does not wait long for a focus.
        AccessibilityNodeInfo field = focusedField(ui, check ? 5 : 20);
        if (field == null) {
            System.out.println("{\"inserted\":false,\"why\":\"no_editable_focus\"}");
            System.out.flush();
            System.exit(3);
        }
        if (field.isPassword()) {
            System.out.println("{\"inserted\":false,\"why\":\"password\"}");
            System.out.flush();
            System.exit(5);
        }
        if (check) {
            System.out.println("{\"inserted\":false,\"typeable\":true}");
            System.out.flush();
            System.exit(0);
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
        // Accessibility writes past a field's length limit, which a person's
        // keys could not: such text is refused whole.
        int limit = field.getMaxTextLength();
        if (limit > 0 && next.length() > limit) {
            System.out.println("{\"inserted\":false,\"why\":\"too_long\",\"limit\":" + limit + "}");
            System.out.flush();
            System.exit(7);
        }
        Bundle set = new Bundle();
        set.putCharSequence(AccessibilityNodeInfo.ACTION_ARGUMENT_SET_TEXT_CHARSEQUENCE, next);
        if (!field.performAction(AccessibilityNodeInfo.ACTION_SET_TEXT, set)) {
            System.out.println("{\"inserted\":false,\"why\":\"field_refused\"}");
            System.out.flush();
            System.exit(4);
        }
        int caret = from + text.length();
        // The cursor goes after the new words first, so the keys that follow land there.
        field.refresh();
        Bundle at = new Bundle();
        at.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_START_INT, caret);
        at.putInt(AccessibilityNodeInfo.ACTION_ARGUMENT_SELECTION_END_INT, caret);
        boolean placed = field.performAction(AccessibilityNodeInfo.ACTION_SET_SELECTION, at);
        // A web page's tree catches up a moment later: the field is read again
        // for a second before its text counts as changed.
        boolean kept = false;
        for (int attempt = 0; attempt < 20 && !kept; attempt++) {
            field.refresh();
            CharSequence now2 = field.getText();
            kept = now2 != null && next.equals(now2.toString());
            if (!kept) Thread.sleep(50);
        }
        if (!kept) {
            // A field that formats what it is given (a phone number, an amount)
            // took it all the same: its text moved. One that kept its old text
            // refused it.
            field.refresh();
            CharSequence after = field.getText();
            if (after != null && !now.equals(after.toString())) {
                System.out.println("{\"inserted\":true,\"reformatted\":true}");
                System.out.flush();
                System.exit(0);
            }
            System.out.println("{\"inserted\":false,\"why\":\"field_kept_its_text\"}");
            System.out.flush();
            System.exit(6);
        }
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

    /**
     * The field holding the input focus, if one is editable; asked again for a
     * second while windows settle. A web page in Chrome answers no input focus
     * to findFocus, though its focused field says so itself, so the tree is
     * walked for it.
     */
    private static AccessibilityNodeInfo focusedField(UiAutomation ui, int attempts) throws InterruptedException {
        for (int attempt = 0; attempt < attempts; attempt++) {
            AccessibilityNodeInfo root = ui.getRootInActiveWindow();
            if (root != null) {
                AccessibilityNodeInfo focus = root.findFocus(AccessibilityNodeInfo.FOCUS_INPUT);
                if (focus == null || !focus.isEditable()) focus = focusedEditable(root);
                if (focus != null) return focus;
            }
            Thread.sleep(50);
        }
        return null;
    }

    /** A focused editable node under this one, breadth first, within a bound. */
    private static AccessibilityNodeInfo focusedEditable(AccessibilityNodeInfo root) {
        java.util.ArrayDeque<AccessibilityNodeInfo> queue = new java.util.ArrayDeque<>();
        queue.add(root);
        for (int seen = 0; !queue.isEmpty() && seen < 4000; seen++) {
            AccessibilityNodeInfo node = queue.poll();
            if (node.isFocused() && node.isEditable()) return node;
            for (int i = 0; i < node.getChildCount(); i++) {
                AccessibilityNodeInfo child = node.getChild(i);
                if (child != null) queue.add(child);
            }
        }
        return null;
    }
}
