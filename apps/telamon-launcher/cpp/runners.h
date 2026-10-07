// KRunner's plugins, as one more source of results (docs/DESIGN.md,
// "Search"). The five plugins the launcher draws itself (services,
// calculator, unitconverter, shell, systemsettings) never run through the
// RunnerManager. Created at the first non-empty query (or by the prewarm
// after startup); a session starts only then. Nothing runs while the panel
// is hidden.
#pragma once

#include <KRunner/QueryMatch>

#include <QHash>
#include <QObject>
#include <QPair>

#include <memory>
#include <optional>

namespace KRunner
{
class RunnerManager;
}

class Runners : public QObject
{
    Q_OBJECT

public:
    // `backend` is the Rust Backend (called by name).
    explicit Runners(QObject *backend, QObject *parent = nullptr);
    ~Runners() override;

    // The panel opens: a session is wanted. It starts with the manager, which
    // exists only after the first non-empty query.
    void setupMatchSession();
    // The panel hid: runners tear down, the current matches are dropped.
    void matchSessionComplete();
    // krunnerrc changed: the new list applies at the next setupMatchSession.
    void reload();
    // Creates the manager without a session: one single-shot after startup.
    void prewarm();
    // True once the manager exists (a runner row can run).
    bool ready() const { return m_manager != nullptr; }

    // The match of the current query, or nothing (unknown or stale).
    std::optional<KRunner::QueryMatch> find(const QString &runnerId, const QString &matchId) const;
    bool run(const KRunner::QueryMatch &match);

public Q_SLOTS:
    void searchStarted(const QString &text);

private:
    KRunner::RunnerManager *manager();
    QStringList allowedRunners() const;
    void onMatches(const QList<KRunner::QueryMatch> &matches);

    QObject *m_backend;
    KRunner::RunnerManager *m_manager = nullptr;
    bool m_session = false;
    bool m_dirty = false;
    int m_serial = 0;
    QHash<QPair<QString, QString>, KRunner::QueryMatch> m_matches;
};
