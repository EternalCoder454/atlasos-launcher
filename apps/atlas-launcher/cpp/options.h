// launcher.conf (written by Settings) and krunnerrc's [Plugins], followed
// live with KConfigWatcher (docs/DESIGN.md, "Interfaces", "Files").
#pragma once

#include <KConfigWatcher>
#include <KSharedConfig>

#include <QObject>

class Options : public QObject
{
    Q_OBJECT
    Q_PROPERTY(bool showRecent READ showRecent NOTIFY changed)
    Q_PROPERTY(bool showRecentFiles READ showRecentFiles NOTIFY changed)

public:
    // `backend` is the Rust Backend (called by name).
    explicit Options(QObject *backend, QObject *parent = nullptr);

    bool showRecent() const;
    bool showRecentFiles() const;

    // Reads both files and hands the result to the backend.
    void apply();

Q_SIGNALS:
    // Emitted after the backend has the new options.
    void changed();
    // Emitted when krunnerrc changed (not launcher.conf): the runners reload.
    void krunnerChanged();

private:
    void reload(bool krunner);

    QObject *m_backend;
    KSharedConfig::Ptr m_launcher;
    KSharedConfig::Ptr m_krunner;
    KConfigWatcher::Ptr m_launcherWatcher;
    KConfigWatcher::Ptr m_krunnerWatcher;
    bool m_showRecent = true;
    bool m_showRecentFiles = true;
};
