BEGIN TRANSACTION;
CREATE TABLE activity (
                    id TEXT NOT NULL REFERENCES installations(id) ON DELETE CASCADE,
                    day TEXT NOT NULL, PRIMARY KEY(id, day)
                );
INSERT INTO "activity" VALUES('65cc47da4135bfec8c7c7a266ed1e373170728d07e09ae5ba68cdb290afc92d8','2026-10-15');
INSERT INTO "activity" VALUES('65cc47da4135bfec8c7c7a266ed1e373170728d07e09ae5ba68cdb290afc92d8','2026-10-25');
INSERT INTO "activity" VALUES('634eb7f6e32fd1689eab9765d7017ef5ab5924b70651bb343478aad06e7bc573','2026-10-20');
CREATE TABLE installations (
                    id TEXT PRIMARY KEY, first_day TEXT NOT NULL,
                    last_day TEXT NOT NULL, version TEXT NOT NULL
                );
INSERT INTO "installations" VALUES('65cc47da4135bfec8c7c7a266ed1e373170728d07e09ae5ba68cdb290afc92d8','2026-10-15','2026-10-25','1.0.1');
INSERT INTO "installations" VALUES('634eb7f6e32fd1689eab9765d7017ef5ab5924b70651bb343478aad06e7bc573','2026-10-20','2026-10-20','1.0.0');
CREATE INDEX activity_day ON activity(day);
COMMIT;
