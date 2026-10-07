// The app catalogue: KService/KSycoca on the GUI thread (an mmap'd database,
// a few ms), handed to the backend as one snapshot and rebuilt when KSycoca
// says the database changed (docs/DESIGN.md, "Threads").
#pragma once

#include <QHash>
#include <QObject>
#include <QString>
#include <QTimer>

class Catalog : public QObject
{
    Q_OBJECT

public:
    // `backend` is the Rust Backend (called by name).
    explicit Catalog(QObject *backend, QObject *parent = nullptr);

    // Builds the list and hands it over. The service list is read here; the
    // per-app file times are statted on a pool thread, and the snapshot goes
    // to the backend when they are in.
    void rebuild();

private:
    QObject *m_backend;
    QTimer m_debounce;
    // Install time per desktop file path, so a rebuild stats only new files.
    QHash<QString, qint64> m_installed;
    // Only the newest rebuild's snapshot is handed over.
    quint64 m_generation = 0;
};
