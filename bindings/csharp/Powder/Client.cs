using System.Runtime.InteropServices;
using System.Text;

namespace Powder;

/// <summary>An engine call failed; the message comes from the native layer.</summary>
public sealed class PowderException : Exception
{
    public PowderException(string message) : base(message) { }
}

/// <summary>
/// An open Powder connection. Thread-safe at the native layer (one serialized
/// connection); dispose to close.
///
/// <code>
/// using var db = Client.Connect("sqlite::memory:");
/// db.Execute("CREATE TABLE t (id INTEGER, name TEXT)");
/// db.Execute("INSERT INTO t VALUES (?, ?)", 1, "alice");
/// var batch = db.Query("SELECT id, name FROM t ORDER BY id");
/// Console.WriteLine(batch["name"].GetString(0));
/// </code>
/// </summary>
public sealed class Client : IDisposable
{
    private IntPtr _handle;
    private int _txDepth;

    private Client(IntPtr handle) => _handle = handle;

    /// <summary>
    /// Open a connection. URLs: <c>sqlite::memory:</c>, <c>sqlite://path</c>,
    /// a bare path, and (with the matching native features) <c>postgres://</c>
    /// / <c>mysql://</c>. Set <c>POWDER_LIB</c> to point at the native
    /// library when it is not on the default search path.
    /// </summary>
    public static Client Connect(string url)
    {
        Native.EnsureLoaded();
        var handle = Native.Connect(url);
        if (handle == IntPtr.Zero)
        {
            throw new PowderException(Native.LastErrorString());
        }
        return new Client(handle);
    }

    /// <summary>Run a non-row statement; returns rows affected.</summary>
    public long Execute(string sql, params object?[] parameters)
    {
        CheckOpen();
        long n = Native.Execute(_handle, sql, ToJson(parameters));
        if (n < 0)
        {
            throw new PowderException(Native.LastErrorString());
        }
        return n;
    }

    /// <summary>Run a query; returns the decoded columnar batch.</summary>
    public Batch Query(string sql, params object?[] parameters)
    {
        CheckOpen();
        var ptr = Native.Query(_handle, sql, ToJson(parameters), out nuint len);
        if (ptr == IntPtr.Zero)
        {
            throw new PowderException(Native.LastErrorString());
        }
        try
        {
            var bytes = new byte[(int)len];
            Marshal.Copy(ptr, bytes, 0, (int)len);
            return Batch.Decode(bytes);
        }
        finally
        {
            Native.FreeBuffer(ptr, len);
        }
    }

    /// <summary>
    /// Run <paramref name="body"/> in a transaction: COMMIT on return,
    /// ROLLBACK on throw. Nested calls use savepoints, so an inner failure
    /// rolls back only its own work.
    /// </summary>
    public void Transaction(Action<Client> body)
    {
        int depth = _txDepth;
        string? savepoint = depth > 0 ? $"powder_sp_{depth}" : null;
        Execute(savepoint != null ? $"SAVEPOINT {savepoint}" : "BEGIN IMMEDIATE");
        _txDepth = depth + 1;
        try
        {
            body(this);
            Execute(savepoint != null ? $"RELEASE {savepoint}" : "COMMIT");
        }
        catch
        {
            try
            {
                if (savepoint != null)
                {
                    Execute($"ROLLBACK TO {savepoint}");
                    Execute($"RELEASE {savepoint}");
                }
                else
                {
                    Execute("ROLLBACK");
                }
            }
            catch (PowderException)
            {
                // Surface the original failure.
            }
            throw;
        }
        finally
        {
            _txDepth = depth;
        }
    }

    public void Dispose()
    {
        if (_handle != IntPtr.Zero)
        {
            Native.Close(_handle);
            _handle = IntPtr.Zero;
        }
    }

    private void CheckOpen()
    {
        if (_handle == IntPtr.Zero)
        {
            throw new PowderException("client is closed");
        }
    }

    /// <summary>The native handle, for same-assembly extensions (the ORM).</summary>
    internal IntPtr Handle
    {
        get
        {
            CheckOpen();
            return _handle;
        }
    }

    /// <summary>
    /// Build the model layer from <c>powder.schema.json</c> text — the same
    /// operation semantics as every other Powder ORM, executed by the shared
    /// Rust engine.
    /// </summary>
    public Orm Orm(string schemaJson) => new(this, schemaJson);

    // -- parameter marshaling: object?[] -> JSON array string ----------------

    private static string ToJson(object?[] parameters)
    {
        if (parameters.Length == 0)
        {
            return "[]";
        }
        var sb = new StringBuilder("[");
        for (int i = 0; i < parameters.Length; i++)
        {
            if (i > 0)
            {
                sb.Append(',');
            }
            AppendJson(sb, parameters[i]);
        }
        return sb.Append(']').ToString();
    }

    private static void AppendJson(StringBuilder sb, object? v)
    {
        switch (v)
        {
            case null:
                sb.Append("null");
                break;
            case bool b:
                sb.Append(b ? "true" : "false");
                break;
            case sbyte or byte or short or ushort or int or uint or long:
                sb.Append(Convert.ToInt64(v).ToString(System.Globalization.CultureInfo.InvariantCulture));
                break;
            case float or double or decimal:
                sb.Append(Convert.ToDouble(v).ToString("R", System.Globalization.CultureInfo.InvariantCulture));
                break;
            case string s:
                AppendJsonString(sb, s);
                break;
            default:
                throw new PowderException($"unsupported parameter type {v.GetType()}");
        }
    }

    private static void AppendJsonString(StringBuilder sb, string s)
    {
        sb.Append('"');
        foreach (char ch in s)
        {
            switch (ch)
            {
                case '"': sb.Append("\\\""); break;
                case '\\': sb.Append("\\\\"); break;
                case '\n': sb.Append("\\n"); break;
                case '\r': sb.Append("\\r"); break;
                case '\t': sb.Append("\\t"); break;
                default:
                    if (ch < 0x20)
                    {
                        sb.Append($"\\u{(int)ch:x4}");
                    }
                    else
                    {
                        sb.Append(ch);
                    }
                    break;
            }
        }
        sb.Append('"');
    }
}
