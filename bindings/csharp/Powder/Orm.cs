using System.Runtime.InteropServices;
using System.Text.Json;
using System.Text.Json.Nodes;

namespace Powder;

/// <summary>
/// The model layer over a <see cref="Client"/>: the same operation semantics
/// as the TS/Python/Go ORMs, executed by the shared Rust engine. Options are
/// anonymous objects (or dictionaries) with the unified keys — <c>where</c>,
/// <c>orderBy</c>, <c>limit</c>, <c>offset</c>, <c>include</c>, <c>join</c>.
///
/// <code>
/// using var orm = db.Orm(schemaJson);           // powder.schema.json text
/// var users = orm.Table("users");
/// users.Create(new { id = 1, name = "alice", score = 9.5, active = true });
/// var rows = users.FindMany(new
/// {
///     where = new { active = true, score = new { gte = 5 } },
///     orderBy = new { score = "desc" },
///     limit = 10,
/// });
/// Console.WriteLine(rows[0]!["name"]);
/// </code>
/// </summary>
public sealed class Orm : IDisposable
{
    private readonly Client _client;
    private IntPtr _schema;

    internal Orm(Client client, string schemaJson)
    {
        _client = client;
        _schema = Native.OrmSchemaNew(schemaJson);
        if (_schema == IntPtr.Zero)
        {
            throw new PowderException(Native.LastErrorString());
        }
    }

    /// <summary>Handle for one table's CRUD surface.</summary>
    public OrmTable Table(string name) => new(this, name);

    public void Dispose()
    {
        if (_schema != IntPtr.Zero)
        {
            Native.OrmSchemaFree(_schema);
            _schema = IntPtr.Zero;
        }
    }

    internal long Execute(string opJson)
    {
        long n = Native.OrmExecute(_client.Handle, SchemaHandle(), opJson);
        if (n < 0)
        {
            throw new PowderException(Native.LastErrorString());
        }
        return n;
    }

    internal JsonNode? FindJson(string opJson)
    {
        var ptr = Native.OrmFindJson(_client.Handle, SchemaHandle(), opJson, out nuint len);
        if (ptr == IntPtr.Zero)
        {
            throw new PowderException(Native.LastErrorString());
        }
        try
        {
            var bytes = new byte[(int)len];
            Marshal.Copy(ptr, bytes, 0, (int)len);
            return JsonNode.Parse(bytes);
        }
        finally
        {
            Native.FreeBuffer(ptr, len);
        }
    }

    private IntPtr SchemaHandle()
    {
        if (_schema == IntPtr.Zero)
        {
            throw new PowderException("orm is disposed");
        }
        return _schema;
    }
}

/// <summary>One table's unified CRUD surface.</summary>
public sealed class OrmTable
{
    private static readonly JsonSerializerOptions JsonOpts = new()
    {
        // Keep anonymous-object property names exactly as written (`user_id`,
        // `AND`, `_count`, ...) — the op spec is case-sensitive.
        PropertyNamingPolicy = null,
    };

    private readonly Orm _orm;
    private readonly string _name;

    internal OrmTable(Orm orm, string name)
    {
        _orm = orm;
        _name = name;
    }

    /// <summary>Rows matching <paramref name="opts"/>; null opts = all rows.</summary>
    public JsonArray FindMany(object? opts = null) =>
        (JsonArray)_orm.FindJson(Op("findMany", opts))!;

    /// <summary>First matching row, or null.</summary>
    public JsonObject? FindFirst(object? opts = null) =>
        _orm.FindJson(Op("findFirst", opts)) as JsonObject;

    /// <summary>Every row.</summary>
    public JsonArray All() => FindMany();

    /// <summary>INSERT one row; missing (nullable) columns are omitted.</summary>
    public long Create(object data) => _orm.Execute(Op("create", new { data }));

    /// <summary>Bulk INSERT (chunked multi-row VALUES); every row must carry
    /// the same columns as the first.</summary>
    public long CreateMany(IEnumerable<object> rows) =>
        _orm.Execute(Op("createMany", new { rows }));

    /// <summary>UPDATE matching rows; returns the affected count.</summary>
    public long Update(object where, object data) =>
        _orm.Execute(Op("update", new { where, data }));

    /// <summary>DELETE matching rows. An empty where is rejected — use
    /// <see cref="DeleteAll"/>.</summary>
    public long Delete(object where) => _orm.Execute(Op("delete", new { where }));

    /// <summary>DELETE every row (explicit opt-in).</summary>
    public long DeleteAll() => _orm.Execute(Op("deleteAll", null));

    /// <summary>COUNT rows matching where (null counts everything).</summary>
    public long Count(object? where = null) =>
        _orm.Execute(Op("count", where == null ? null : new { where }));

    /// <summary>Whether at least one row matches.</summary>
    public bool Exists(object? where = null) =>
        FindFirst(where == null ? new { limit = 1 } : new { where, limit = 1 }) != null;

    /// <summary>SUM/AVG/MIN/MAX over one column; null when no rows match.</summary>
    public double? Aggregate(string fn, string column, object? where = null)
    {
        var v = _orm.FindJson(Op("aggregate",
            where == null ? new { fn, column } : new { fn, column, where }));
        return v == null ? null : v.GetValue<double>();
    }

    /// <summary>GROUP BY with aggregates (<c>by</c>, <c>count</c>, <c>sum</c>,
    /// <c>avg</c>, <c>min</c>, <c>max</c>, <c>having</c>, <c>orderBy</c>, ...);
    /// aggregates come back aliased <c>_count</c>, <c>_sum_&lt;col&gt;</c>, ....</summary>
    public JsonArray GroupBy(object opts) => (JsonArray)_orm.FindJson(Op("groupBy", opts))!;

    private string Op(string op, object? opts)
    {
        var node = opts == null
            ? new JsonObject()
            : (JsonObject)JsonSerializer.SerializeToNode(opts, JsonOpts)!;
        node["op"] = op;
        node["table"] = _name;
        return node.ToJsonString(JsonOpts);
    }
}
