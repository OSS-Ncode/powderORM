// powder.hpp — header-only C++17 wrapper over the Powder C ABI.
//
// RAII everywhere: Client closes its connection, Batch frees the native PCB
// buffer. The PCB decoder reads values straight out of the buffer (columnar,
// little-endian); nothing is materialized until you ask for it.
//
//   powder::Client db("sqlite::memory:");
//   db.execute("CREATE TABLE t (id INTEGER, name TEXT)");
//   db.execute("INSERT INTO t VALUES (?, ?)", {int64_t{1}, "alice"});
//   powder::Batch b = db.query("SELECT id, name FROM t ORDER BY id");
//   for (size_t r = 0; r < b.num_rows(); ++r)
//     std::cout << b["id"].i64(r) << " " << b["name"].str(r) << "\n";

#ifndef POWDER_HPP
#define POWDER_HPP

#include <cstdint>
#include <cstring>
#include <memory>
#include <stdexcept>
#include <string>
#include <string_view>
#include <variant>
#include <vector>

#include "../../c/include/powder.h"

namespace powder {

/// Thrown for every failing engine call; carries the engine's message.
class Error : public std::runtime_error {
public:
    explicit Error(const std::string& what) : std::runtime_error(what) {}
};

namespace detail {
inline std::string last_error() {
    const char* msg = powder_last_error();
    return msg ? std::string(msg) : std::string("unknown powder error");
}
} // namespace detail

/// A bound SQL parameter: NULL, int64, double, bool, or text.
using Param = std::variant<std::nullptr_t, int64_t, double, bool, std::string, const char*>;

enum class DataType : uint8_t { Int64 = 0, Float64 = 1, Bool = 2, Utf8 = 3 };

/// One decoded PCB column — a lightweight view into the batch's buffer.
class Column {
public:
    const std::string& name() const { return name_; }
    DataType type() const { return type_; }
    size_t length() const { return length_; }

    /// Whether the slot holds a value (vs. SQL NULL).
    bool is_valid(size_t row) const {
        if (validity_off_ < 0) return true;
        const uint8_t b = data_[static_cast<size_t>(validity_off_) + (row >> 3)];
        return (b & (1u << (row & 7))) != 0;
    }

    int64_t i64(size_t row) const {
        int64_t v;
        std::memcpy(&v, data_ + buf1_off_ + row * 8, 8);
        return v;
    }

    double f64(size_t row) const {
        double v;
        std::memcpy(&v, data_ + buf1_off_ + row * 8, 8);
        return v;
    }

    bool boolean(size_t row) const {
        const uint8_t b = data_[buf1_off_ + (row >> 3)];
        return (b & (1u << (row & 7))) != 0;
    }

    std::string_view str(size_t row) const {
        const uint32_t start = utf8_offsets_[row];
        const uint32_t end = utf8_offsets_[row + 1];
        return std::string_view(reinterpret_cast<const char*>(data_ + buf2_off_ + start),
                                end - start);
    }

private:
    friend class Batch;
    std::string name_;
    DataType type_ = DataType::Int64;
    size_t length_ = 0;
    const uint8_t* data_ = nullptr; // whole PCB buffer
    ptrdiff_t validity_off_ = -1;
    size_t buf1_off_ = 0;
    size_t buf2_off_ = 0;
    std::vector<uint32_t> utf8_offsets_; // only for Utf8 columns
};

/// A decoded, owning result set. Move-only; frees the native buffer on
/// destruction.
class Batch {
public:
    Batch(unsigned char* buf, size_t len) : buf_(buf), len_(len) { decode(); }

    Batch(Batch&& other) noexcept { *this = std::move(other); }
    Batch& operator=(Batch&& other) noexcept {
        if (this != &other) {
            release();
            buf_ = other.buf_;
            len_ = other.len_;
            num_rows_ = other.num_rows_;
            columns_ = std::move(other.columns_);
            other.buf_ = nullptr;
            other.len_ = 0;
        }
        return *this;
    }
    Batch(const Batch&) = delete;
    Batch& operator=(const Batch&) = delete;
    ~Batch() { release(); }

    size_t num_rows() const { return num_rows_; }
    const std::vector<Column>& columns() const { return columns_; }

    const Column& operator[](std::string_view name) const {
        for (const auto& c : columns_) {
            if (c.name() == name) return c;
        }
        throw Error("no such column: " + std::string(name));
    }

private:
    void release() {
        if (buf_) {
            powder_free_buffer(buf_, len_);
            buf_ = nullptr;
        }
    }

    uint32_t u32(size_t off) const {
        uint32_t v;
        std::memcpy(&v, buf_ + off, 4);
        return v;
    }
    uint16_t u16(size_t off) const {
        uint16_t v;
        std::memcpy(&v, buf_ + off, 2);
        return v;
    }

    void decode() {
        constexpr uint32_t kMagic = 0x31424350; // "PCB1" little-endian
        constexpr size_t kColDir = 40;
        if (len_ < 24 || u32(0) != kMagic) throw Error("not a PCB buffer (bad magic)");
        if (u16(4) != 1) throw Error("unsupported PCB version " + std::to_string(u16(4)));
        const uint32_t num_columns = u32(8);
        num_rows_ = u32(12);
        const uint32_t dir_off = u32(16);

        columns_.reserve(num_columns);
        for (uint32_t c = 0; c < num_columns; ++c) {
            const size_t d = dir_off + c * kColDir;
            Column col;
            const uint32_t name_off = u32(d);
            const uint32_t name_len = u32(d + 4);
            col.name_.assign(reinterpret_cast<const char*>(buf_ + name_off), name_len);
            const uint8_t dtype = buf_[d + 8];
            if (dtype > 3) throw Error("unsupported PCB type code " + std::to_string(dtype));
            col.type_ = static_cast<DataType>(dtype);
            const bool has_validity = (buf_[d + 9] & 1) != 0;
            col.validity_off_ = has_validity ? static_cast<ptrdiff_t>(u32(d + 12)) : -1;
            col.buf1_off_ = u32(d + 20);
            col.buf2_off_ = u32(d + 28);
            col.length_ = num_rows_;
            col.data_ = buf_;
            if (col.type_ == DataType::Utf8) {
                col.utf8_offsets_.resize(num_rows_ + 1);
                std::memcpy(col.utf8_offsets_.data(), buf_ + col.buf1_off_,
                            (num_rows_ + 1) * 4);
            }
            columns_.push_back(std::move(col));
        }
    }

    unsigned char* buf_ = nullptr;
    size_t len_ = 0;
    size_t num_rows_ = 0;
    std::vector<Column> columns_;
};

namespace detail {
inline void append_json(std::string& out, const Param& p) {
    struct V {
        std::string& out;
        void operator()(std::nullptr_t) const { out += "null"; }
        void operator()(int64_t v) const { out += std::to_string(v); }
        void operator()(double v) const {
            char buf[32];
            std::snprintf(buf, sizeof buf, "%.17g", v);
            out += buf;
        }
        void operator()(bool v) const { out += v ? "true" : "false"; }
        void operator()(const std::string& s) const { escape(s); }
        void operator()(const char* s) const { escape(s ? s : ""); }
        void escape(std::string_view s) const {
            out += '"';
            for (char ch : s) {
                switch (ch) {
                    case '"': out += "\\\""; break;
                    case '\\': out += "\\\\"; break;
                    case '\n': out += "\\n"; break;
                    case '\r': out += "\\r"; break;
                    case '\t': out += "\\t"; break;
                    default:
                        if (static_cast<unsigned char>(ch) < 0x20) {
                            char esc[8];
                            std::snprintf(esc, sizeof esc, "\\u%04x", ch);
                            out += esc;
                        } else {
                            out += ch;
                        }
                }
            }
            out += '"';
        }
    };
    std::visit(V{out}, p);
}

inline std::string to_json(const std::vector<Param>& params) {
    std::string out = "[";
    for (size_t i = 0; i < params.size(); ++i) {
        if (i) out += ',';
        append_json(out, params[i]);
    }
    out += ']';
    return out;
}
} // namespace detail

/// An open Powder connection. Move-only; closes on destruction.
class Client {
public:
    explicit Client(const std::string& url) : handle_(powder_connect(url.c_str())) {
        if (!handle_) throw Error(detail::last_error());
    }

    Client(Client&& other) noexcept : handle_(other.handle_) { other.handle_ = nullptr; }
    Client& operator=(Client&& other) noexcept {
        if (this != &other) {
            close();
            handle_ = other.handle_;
            other.handle_ = nullptr;
        }
        return *this;
    }
    Client(const Client&) = delete;
    Client& operator=(const Client&) = delete;
    ~Client() { close(); }

    /// INSERT/UPDATE/DDL; returns rows affected.
    int64_t execute(const std::string& sql, const std::vector<Param>& params = {}) {
        check_open();
        const std::string json = detail::to_json(params);
        const int64_t n = powder_execute(handle_, sql.c_str(), json.c_str());
        if (n < 0) throw Error(detail::last_error());
        return n;
    }

    /// Run a query; returns the decoded columnar batch.
    Batch query(const std::string& sql, const std::vector<Param>& params = {}) {
        check_open();
        const std::string json = detail::to_json(params);
        size_t len = 0;
        unsigned char* buf = powder_query(handle_, sql.c_str(), json.c_str(), &len);
        if (!buf) throw Error(detail::last_error());
        return Batch(buf, len);
    }

    /// Run `fn` inside a transaction; nested calls use savepoints.
    template <typename Fn>
    void transaction(Fn&& fn) {
        const int depth = tx_depth_;
        const std::string sp = depth > 0 ? "powder_sp_" + std::to_string(depth) : "";
        execute(depth > 0 ? "SAVEPOINT " + sp : "BEGIN IMMEDIATE");
        ++tx_depth_;
        try {
            fn(*this);
            execute(depth > 0 ? "RELEASE " + sp : "COMMIT");
        } catch (...) {
            try {
                if (depth > 0) {
                    execute("ROLLBACK TO " + sp);
                    execute("RELEASE " + sp);
                } else {
                    execute("ROLLBACK");
                }
            } catch (...) {
                // surface the original failure
            }
            --tx_depth_;
            throw;
        }
        --tx_depth_;
    }

    void close() {
        if (handle_) {
            powder_close(handle_);
            handle_ = nullptr;
        }
    }

    /// The raw C-ABI handle (for the ORM layer and other extensions).
    PowderClient* native_handle() const { return handle_; }

private:
    void check_open() const {
        if (!handle_) throw Error("client is closed");
    }

    PowderClient* handle_ = nullptr;
    int tx_depth_ = 0;
};

class OrmTable;

/// The model layer over a Client: the same operation semantics as the other
/// Powder ORMs, executed by the shared Rust engine. Options cross as JSON
/// object strings (build them with your JSON library of choice — or string
/// literals); rows come back as a JSON string.
///
///   powder::Orm orm(db, schema_json);           // powder.schema.json text
///   auto users = orm.table("users");
///   users.create(R"({"id":1,"name":"alice","score":9.5,"active":true})");
///   std::string rows = users.find_many(
///       R"({"where":{"active":true,"score":{"gte":5}},
///           "orderBy":{"score":"desc"},"limit":10})");
class Orm {
public:
    Orm(Client& client, const std::string& schema_json)
        : client_(&client), schema_(powder_orm_schema_new(schema_json.c_str())) {
        if (!schema_) throw Error(detail::last_error());
    }

    Orm(Orm&& other) noexcept : client_(other.client_), schema_(other.schema_) {
        other.schema_ = nullptr;
    }
    Orm& operator=(Orm&& other) noexcept {
        if (this != &other) {
            release();
            client_ = other.client_;
            schema_ = other.schema_;
            other.schema_ = nullptr;
        }
        return *this;
    }
    Orm(const Orm&) = delete;
    Orm& operator=(const Orm&) = delete;
    ~Orm() { release(); }

    /// Handle for one table's CRUD surface.
    inline OrmTable table(std::string name);

    /// Run a raw op object (`{"op":"findMany","table":"users",...}`) and
    /// return its JSON result — the low-level unified spec.
    std::string find_json(const std::string& op_json) {
        check();
        size_t len = 0;
        unsigned char* buf =
            powder_orm_find_json(client_->native_handle(), schema_, op_json.c_str(), &len);
        if (!buf) throw Error(detail::last_error());
        std::string out(reinterpret_cast<const char*>(buf), len);
        powder_free_buffer(buf, len);
        return out;
    }

    /// Run a raw mutation/count op; returns the affected/row count.
    int64_t execute(const std::string& op_json) {
        check();
        const int64_t n =
            powder_orm_execute(client_->native_handle(), schema_, op_json.c_str());
        if (n < 0) throw Error(detail::last_error());
        return n;
    }

private:
    void check() const {
        if (!schema_ || !client_->native_handle()) throw Error("orm or client is closed");
    }
    void release() {
        if (schema_) {
            powder_orm_schema_free(schema_);
            schema_ = nullptr;
        }
    }

    Client* client_ = nullptr;
    PowderOrmSchema* schema_ = nullptr;
};

/// One table's unified CRUD surface. Option arguments are JSON object
/// strings using the same keys as the TS/Python/Go ORMs (`where`, `orderBy`,
/// `limit`, `offset`, `include`, `join`, ...).
class OrmTable {
public:
    OrmTable(Orm& orm, std::string name) : orm_(&orm), name_(std::move(name)) {}

    /// Rows matching `opts` as a JSON array string. `opts` may be "" or "{}".
    std::string find_many(const std::string& opts = "{}") {
        return orm_->find_json(op("findMany", opts));
    }

    /// First matching row as JSON — an object, or "null".
    std::string find_first(const std::string& opts = "{}") {
        return orm_->find_json(op("findFirst", opts));
    }

    /// INSERT one row: `create(R"({"id":1,"name":"alice"})")`.
    int64_t create(const std::string& data_json) {
        return orm_->execute(op("create", "{\"data\":" + data_json + "}"));
    }

    /// Bulk INSERT: `create_many(R"([{...},{...}])")` (chunked multi-row VALUES).
    int64_t create_many(const std::string& rows_json) {
        return orm_->execute(op("createMany", "{\"rows\":" + rows_json + "}"));
    }

    /// UPDATE matching rows; returns the affected count.
    int64_t update(const std::string& where_json, const std::string& data_json) {
        return orm_->execute(
            op("update", "{\"where\":" + where_json + ",\"data\":" + data_json + "}"));
    }

    /// DELETE matching rows (an empty where is rejected — use remove_all).
    int64_t remove(const std::string& where_json) {
        return orm_->execute(op("delete", "{\"where\":" + where_json + "}"));
    }

    /// DELETE every row (explicit opt-in).
    int64_t remove_all() { return orm_->execute(op("deleteAll", "{}")); }

    /// COUNT rows matching where ("{}" counts everything).
    int64_t count(const std::string& where_json = "{}") {
        return orm_->execute(op("count", "{\"where\":" + where_json + "}"));
    }

    /// Whether at least one row matches.
    bool exists(const std::string& where_json = "{}") {
        return orm_->find_json(op("findFirst", "{\"where\":" + where_json + ",\"limit\":1}")) !=
               "null";
    }

    /// SUM/AVG/MIN/MAX over one column; JSON number, or "null" when no rows.
    std::string aggregate(const std::string& fn, const std::string& column,
                          const std::string& where_json = "{}") {
        return orm_->find_json(op("aggregate", "{\"fn\":\"" + fn + "\",\"column\":\"" + column +
                                                   "\",\"where\":" + where_json + "}"));
    }

    /// GROUP BY with aggregates (`by`, `count`, `sum`, `avg`, `min`, `max`,
    /// `having`, `orderBy`, ...); JSON array with `_count`/`_sum_<col>` aliases.
    std::string group_by(const std::string& opts) {
        return orm_->find_json(op("groupBy", opts));
    }

private:
    /// Splice `{"op":...,"table":...}` together with the caller's options.
    std::string op(const char* name, const std::string& opts) const {
        std::string out = "{\"op\":\"";
        out += name;
        out += "\",\"table\":\"";
        out += name_;
        out += '"';
        // Merge the option object's members, if any.
        const size_t open = opts.find('{');
        const size_t close = opts.rfind('}');
        if (open != std::string::npos && close != std::string::npos && close > open + 1) {
            const std::string inner = opts.substr(open + 1, close - open - 1);
            if (inner.find_first_not_of(" \t\r\n") != std::string::npos) {
                out += ',';
                out += inner;
            }
        }
        out += '}';
        return out;
    }

    Orm* orm_;
    std::string name_;
};

inline OrmTable Orm::table(std::string name) { return OrmTable(*this, std::move(name)); }

} // namespace powder

#endif // POWDER_HPP
