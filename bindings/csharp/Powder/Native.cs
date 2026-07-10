using System.Reflection;
using System.Runtime.InteropServices;

namespace Powder;

/// <summary>
/// Raw P/Invoke surface over the powder_ffi C ABI.
///
/// The shared library is resolved in this order: the <c>POWDER_LIB</c>
/// environment variable (full path), then the OS loader's default search
/// (<c>powder_ffi.dll</c> / <c>libpowder_ffi.so</c> / <c>.dylib</c> next to
/// the app or on PATH).
/// </summary>
internal static partial class Native
{
    private const string Lib = "powder_ffi";

    static Native()
    {
        NativeLibrary.SetDllImportResolver(Assembly.GetExecutingAssembly(), Resolve);
    }

    private static IntPtr Resolve(string name, Assembly assembly, DllImportSearchPath? path)
    {
        if (name != Lib)
        {
            return IntPtr.Zero;
        }
        var env = Environment.GetEnvironmentVariable("POWDER_LIB");
        if (!string.IsNullOrEmpty(env) && NativeLibrary.TryLoad(env, out var fromEnv))
        {
            return fromEnv;
        }
        return NativeLibrary.TryLoad(Lib, assembly, path, out var handle) ? handle : IntPtr.Zero;
    }

    /// Touching any member runs the static constructor — call before first use.
    internal static void EnsureLoaded() { }

    [DllImport(Lib, EntryPoint = "powder_connect")]
    internal static extern IntPtr Connect([MarshalAs(UnmanagedType.LPUTF8Str)] string url);

    [DllImport(Lib, EntryPoint = "powder_execute")]
    internal static extern long Execute(
        IntPtr client,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string sql,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string paramsJson);

    [DllImport(Lib, EntryPoint = "powder_query")]
    internal static extern IntPtr Query(
        IntPtr client,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string sql,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string paramsJson,
        out nuint outLen);

    [DllImport(Lib, EntryPoint = "powder_free_buffer")]
    internal static extern void FreeBuffer(IntPtr ptr, nuint len);

    [DllImport(Lib, EntryPoint = "powder_last_error")]
    internal static extern IntPtr LastError();

    [DllImport(Lib, EntryPoint = "powder_close")]
    internal static extern void Close(IntPtr client);

    [DllImport(Lib, EntryPoint = "powder_orm_schema_new")]
    internal static extern IntPtr OrmSchemaNew([MarshalAs(UnmanagedType.LPUTF8Str)] string schemaJson);

    [DllImport(Lib, EntryPoint = "powder_orm_schema_free")]
    internal static extern void OrmSchemaFree(IntPtr schema);

    [DllImport(Lib, EntryPoint = "powder_orm_execute")]
    internal static extern long OrmExecute(
        IntPtr client,
        IntPtr schema,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string opJson);

    [DllImport(Lib, EntryPoint = "powder_orm_find_json")]
    internal static extern IntPtr OrmFindJson(
        IntPtr client,
        IntPtr schema,
        [MarshalAs(UnmanagedType.LPUTF8Str)] string opJson,
        out nuint outLen);

    internal static string LastErrorString()
    {
        var p = LastError();
        return p == IntPtr.Zero
            ? "unknown powder error"
            : Marshal.PtrToStringUTF8(p) ?? "unknown powder error";
    }
}
