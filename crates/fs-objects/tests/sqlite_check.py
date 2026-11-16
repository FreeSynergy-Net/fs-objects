"""Standard SQLite must read a store file: integrity_check ok and no orphan autoindex (run as `python3 -I`)."""
import sqlite3
import sys

con = sqlite3.connect("file:" + sys.argv[1] + "?mode=ro", uri=True)
assert con.execute("PRAGMA integrity_check").fetchall() == [("ok",)], "integrity_check"
orphans = con.execute(
    "SELECT name FROM sqlite_schema WHERE type='index' AND sql IS NULL "
    "AND name NOT LIKE 'sqlite_autoindex_' || tbl_name || '_%'"
).fetchall()
assert not orphans, orphans
print("ok", con.execute("SELECT count(*) FROM sqlite_schema").fetchone()[0])
