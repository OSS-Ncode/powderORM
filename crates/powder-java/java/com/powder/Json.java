package com.powder;

import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

/**
 * Minimal JSON reader/writer for the ORM boundary — no dependencies, just the
 * shapes the engine exchanges: objects ({@link LinkedHashMap}, insertion
 * order preserved), arrays ({@link ArrayList}), strings, {@link Long} /
 * {@link Double} numbers, booleans, and {@code null}.
 */
final class Json {
    private Json() {}

    // -- writing --------------------------------------------------------------

    static String write(Object v) {
        StringBuilder sb = new StringBuilder();
        append(sb, v);
        return sb.toString();
    }

    private static void append(StringBuilder sb, Object v) {
        if (v == null) {
            sb.append("null");
        } else if (v instanceof String) {
            appendString(sb, (String) v);
        } else if (v instanceof Boolean) {
            sb.append(((Boolean) v) ? "true" : "false");
        } else if (v instanceof Double || v instanceof Float) {
            double d = ((Number) v).doubleValue();
            if (Double.isNaN(d) || Double.isInfinite(d)) {
                throw new IllegalArgumentException("cannot encode non-finite number");
            }
            sb.append(v);
        } else if (v instanceof Number) {
            sb.append(v);
        } else if (v instanceof Map) {
            sb.append('{');
            boolean first = true;
            for (Map.Entry<?, ?> e : ((Map<?, ?>) v).entrySet()) {
                if (!first) {
                    sb.append(',');
                }
                first = false;
                appendString(sb, String.valueOf(e.getKey()));
                sb.append(':');
                append(sb, e.getValue());
            }
            sb.append('}');
        } else if (v instanceof Iterable) {
            sb.append('[');
            boolean first = true;
            for (Object e : (Iterable<?>) v) {
                if (!first) {
                    sb.append(',');
                }
                first = false;
                append(sb, e);
            }
            sb.append(']');
        } else if (v instanceof Object[]) {
            sb.append('[');
            Object[] arr = (Object[]) v;
            for (int i = 0; i < arr.length; i++) {
                if (i > 0) {
                    sb.append(',');
                }
                append(sb, arr[i]);
            }
            sb.append(']');
        } else {
            throw new IllegalArgumentException("unsupported JSON value: " + v.getClass().getName());
        }
    }

    private static void appendString(StringBuilder sb, String s) {
        sb.append('"');
        for (int i = 0; i < s.length(); i++) {
            char c = s.charAt(i);
            switch (c) {
                case '"': sb.append("\\\""); break;
                case '\\': sb.append("\\\\"); break;
                case '\n': sb.append("\\n"); break;
                case '\r': sb.append("\\r"); break;
                case '\t': sb.append("\\t"); break;
                default:
                    if (c < 0x20) {
                        sb.append(String.format("\\u%04x", (int) c));
                    } else {
                        sb.append(c);
                    }
            }
        }
        sb.append('"');
    }

    // -- reading (input is engine-produced, well-formed JSON) ------------------

    static Object read(String text) {
        Parser p = new Parser(text);
        Object v = p.value();
        p.skipWs();
        if (!p.atEnd()) {
            throw new IllegalStateException("trailing JSON content at " + p.pos);
        }
        return v;
    }

    private static final class Parser {
        final String s;
        int pos = 0;

        Parser(String s) {
            this.s = s;
        }

        boolean atEnd() {
            return pos >= s.length();
        }

        void skipWs() {
            while (pos < s.length() && Character.isWhitespace(s.charAt(pos))) {
                pos++;
            }
        }

        Object value() {
            skipWs();
            char c = s.charAt(pos);
            switch (c) {
                case '{': return object();
                case '[': return array();
                case '"': return string();
                case 't': expect("true"); return Boolean.TRUE;
                case 'f': expect("false"); return Boolean.FALSE;
                case 'n': expect("null"); return null;
                default: return number();
            }
        }

        void expect(String word) {
            if (!s.startsWith(word, pos)) {
                throw new IllegalStateException("bad JSON literal at " + pos);
            }
            pos += word.length();
        }

        Map<String, Object> object() {
            Map<String, Object> out = new LinkedHashMap<>();
            pos++; // {
            skipWs();
            if (s.charAt(pos) == '}') {
                pos++;
                return out;
            }
            while (true) {
                skipWs();
                String key = string();
                skipWs();
                pos++; // :
                out.put(key, value());
                skipWs();
                char c = s.charAt(pos++);
                if (c == '}') {
                    return out;
                }
                // else ','
            }
        }

        List<Object> array() {
            List<Object> out = new ArrayList<>();
            pos++; // [
            skipWs();
            if (s.charAt(pos) == ']') {
                pos++;
                return out;
            }
            while (true) {
                out.add(value());
                skipWs();
                char c = s.charAt(pos++);
                if (c == ']') {
                    return out;
                }
                // else ','
            }
        }

        String string() {
            pos++; // "
            StringBuilder sb = new StringBuilder();
            while (true) {
                char c = s.charAt(pos++);
                if (c == '"') {
                    return sb.toString();
                }
                if (c != '\\') {
                    sb.append(c);
                    continue;
                }
                char esc = s.charAt(pos++);
                switch (esc) {
                    case '"': sb.append('"'); break;
                    case '\\': sb.append('\\'); break;
                    case '/': sb.append('/'); break;
                    case 'n': sb.append('\n'); break;
                    case 'r': sb.append('\r'); break;
                    case 't': sb.append('\t'); break;
                    case 'b': sb.append('\b'); break;
                    case 'f': sb.append('\f'); break;
                    case 'u':
                        sb.append((char) Integer.parseInt(s.substring(pos, pos + 4), 16));
                        pos += 4;
                        break;
                    default:
                        throw new IllegalStateException("bad JSON escape at " + pos);
                }
            }
        }

        Object number() {
            int start = pos;
            boolean floating = false;
            while (pos < s.length()) {
                char c = s.charAt(pos);
                if (c == '-' || c == '+' || (c >= '0' && c <= '9')) {
                    pos++;
                } else if (c == '.' || c == 'e' || c == 'E') {
                    floating = true;
                    pos++;
                } else {
                    break;
                }
            }
            String text = s.substring(start, pos);
            return floating ? (Object) Double.parseDouble(text) : (Object) Long.parseLong(text);
        }
    }
}
