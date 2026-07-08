import { sqliteTable, integer, text, real } from "drizzle-orm/sqlite-core";

export const benchUsers = sqliteTable("bench_users", {
  id: integer("id").primaryKey({ autoIncrement: false }),
  name: text("name").notNull(),
  score: real("score"),
});
