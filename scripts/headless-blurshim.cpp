// An LD_PRELOAD shim for headless runs (scripts/headless-look.sh). The
// private KWin of a headless run draws with QPainter and offers no blur, so
// KWindowEffects never gets as far as a blur request. This stands in for the
// two calls the launcher and Telamon.Ui make:
//   isEffectAvailable(BlurBehind): true when the file named by
//       $BLURSHIM_STATE starts with "1" (read on each call, so a run can
//       switch the compositor's blur on and off, as the Blur effect does);
//   enableBlurBehind(window, enable, region): appended to $BLURSHIM_LOG with
//       the window's size and the region's rectangles, in surface pixels.
// Nothing here is installed or shipped.
#include <KWindowEffects>

#include <QRegion>
#include <QWindow>

#include <cstdio>
#include <cstdlib>

namespace KWindowEffects
{
bool isEffectAvailable(Effect effect)
{
    if (effect != BlurBehind) {
        return false;
    }
    const char *state = std::getenv("BLURSHIM_STATE");
    FILE *f = state ? std::fopen(state, "r") : nullptr;
    if (!f) {
        return false;
    }
    const int c = std::fgetc(f);
    std::fclose(f);
    return c == '1';
}

void enableBlurBehind(QWindow *window, bool enable, const QRegion &region)
{
    const char *path = std::getenv("BLURSHIM_LOG");
    FILE *f = path ? std::fopen(path, "a") : nullptr;
    if (!f || !window) {
        if (f) {
            std::fclose(f);
        }
        return;
    }
    std::fprintf(f, "blur enable=%d window=%dx%d rects=%d", enable ? 1 : 0, window->width(), window->height(), int(region.rectCount()));
    for (const QRect &r : region) {
        std::fprintf(f, " %d,%d,%d,%d", r.x(), r.y(), r.width(), r.height());
    }
    std::fputc('\n', f);
    std::fclose(f);
}
}
