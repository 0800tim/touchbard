// touchbar-milkdrop: render Milkdrop (libprojectM 4) offscreen and stream
// frames in the Touch Bar plugin format.
//
//   parec ... | touchbar-milkdrop WIDTH HEIGHT PRESET_DIR [TEXTURE_DIR] [FPS]
//
// stdin:  float32 little-endian stereo PCM at 44.1 kHz (non-blocking; silence is fine)
// stdout: {"t":"frame","w":W,"h":H,"len":N}\n followed by N bytes of BGRA, top row first
// SIGUSR1: next preset (hard cut)

#define GL_GLEXT_PROTOTYPES 1
#include <EGL/egl.h>
#include <EGL/eglext.h>
#include <GL/gl.h>
#include <GL/glext.h>
#include <projectM-4/playlist.h>
#include <projectM-4/projectM.h>

#include <errno.h>
#include <fcntl.h>
#include <signal.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

static volatile sig_atomic_t next_preset = 0;
static void on_usr1(int sig) { (void)sig; next_preset = 1; }

static void die(const char *what) {
    fprintf(stderr, "touchbar-milkdrop: %s\n", what);
    exit(1);
}

static double now_s(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec + ts.tv_nsec / 1e9;
}

static void write_all(const void *buf, size_t len) {
    const char *p = buf;
    while (len > 0) {
        ssize_t n = write(STDOUT_FILENO, p, len);
        if (n < 0) {
            if (errno == EINTR) continue;
            exit(0); // the agent went away
        }
        p += n;
        len -= (size_t)n;
    }
}

int main(int argc, char **argv) {
    if (argc < 4) die("usage: touchbar-milkdrop W H PRESET_DIR [TEXTURE_DIR] [FPS]");
    int w = atoi(argv[1]), h = atoi(argv[2]);
    const char *presets = argv[3];
    const char *textures = argc > 4 ? argv[4] : NULL;
    int fps = argc > 5 ? atoi(argv[5]) : 30;
    if (w <= 0 || h <= 0 || fps <= 0) die("bad size or fps");

    // --- EGL: a surfaceless desktop-GL context on the GPU ---
    PFNEGLGETPLATFORMDISPLAYEXTPROC get_display =
        (PFNEGLGETPLATFORMDISPLAYEXTPROC)eglGetProcAddress("eglGetPlatformDisplayEXT");
    EGLDisplay dpy = get_display ? get_display(EGL_PLATFORM_SURFACELESS_MESA, EGL_DEFAULT_DISPLAY, NULL)
                                 : eglGetDisplay(EGL_DEFAULT_DISPLAY);
    if (dpy == EGL_NO_DISPLAY || !eglInitialize(dpy, NULL, NULL)) die("no EGL display");
    if (!eglBindAPI(EGL_OPENGL_API)) die("no desktop GL");
    EGLint cfg_attrs[] = {EGL_RENDERABLE_TYPE, EGL_OPENGL_BIT, EGL_SURFACE_TYPE, EGL_PBUFFER_BIT,
                          EGL_RED_SIZE, 8, EGL_GREEN_SIZE, 8, EGL_BLUE_SIZE, 8, EGL_NONE};
    EGLConfig cfg;
    EGLint n = 0;
    if (!eglChooseConfig(dpy, cfg_attrs, &cfg, 1, &n) || n < 1) die("no EGL pbuffer config");
    EGLint ctx_attrs[] = {EGL_CONTEXT_MAJOR_VERSION, 3, EGL_CONTEXT_MINOR_VERSION, 3,
                          EGL_CONTEXT_OPENGL_PROFILE_MASK, EGL_CONTEXT_OPENGL_CORE_PROFILE_BIT, EGL_NONE};
    EGLContext ctx = eglCreateContext(dpy, cfg, EGL_NO_CONTEXT, ctx_attrs);
    if (ctx == EGL_NO_CONTEXT) die("no GL 3.3 context");
    // projectM always draws to the default framebuffer, so give it a real one:
    // a pbuffer the size of the strip, read straight back.
    EGLint pb_attrs[] = {EGL_WIDTH, w, EGL_HEIGHT, h, EGL_NONE};
    EGLSurface pb = eglCreatePbufferSurface(dpy, cfg, pb_attrs);
    if (pb == EGL_NO_SURFACE) die("no pbuffer");
    if (!eglMakeCurrent(dpy, pb, pb, ctx)) die("eglMakeCurrent failed");
    glViewport(0, 0, w, h);

    // --- projectM ---
    projectm_handle pm = projectm_create();
    if (!pm) die("projectm_create failed");
    projectm_set_window_size(pm, (size_t)w, (size_t)h);
    projectm_set_fps(pm, fps);
    projectm_set_mesh_size(pm, 64, 16); // a wide, short strip: fewer rows
    projectm_set_preset_duration(pm, 40);
    projectm_set_soft_cut_duration(pm, 4);
    projectm_set_hard_cut_enabled(pm, false);
    projectm_set_beat_sensitivity(pm, 1.2f);
    if (textures) {
        const char *paths[] = {textures};
        projectm_set_texture_search_paths(pm, paths, 1);
    }
    projectm_playlist_handle pl = projectm_playlist_create(pm);
    projectm_playlist_set_shuffle(pl, true);
    if (projectm_playlist_add_path(pl, presets, true, false) == 0) die("no presets found");
    projectm_playlist_play_next(pl, true);

    signal(SIGUSR1, on_usr1);
    signal(SIGPIPE, SIG_IGN);
    fcntl(STDIN_FILENO, F_SETFL, fcntl(STDIN_FILENO, F_GETFL) | O_NONBLOCK);

    size_t frame_len = (size_t)w * h * 4;
    unsigned char *pixels = malloc(frame_len), *flipped = malloc(frame_len);
    float pcm[4096];
    char header[128];
    int hlen = snprintf(header, sizeof header, "{\"t\":\"frame\",\"w\":%d,\"h\":%d,\"len\":%zu}\n", w, h, frame_len);
    double period = 1.0 / fps, next = now_s();

    for (;;) {
        // Feed whatever audio has arrived since the last frame.
        for (;;) {
            ssize_t got = read(STDIN_FILENO, pcm, sizeof pcm);
            if (got <= 0) break;
            projectm_pcm_add_float(pm, pcm, (unsigned)(got / (sizeof(float) * 2)), PROJECTM_STEREO);
        }
        if (next_preset) {
            next_preset = 0;
            projectm_playlist_play_next(pl, true);
        }

        glViewport(0, 0, w, h);
        projectm_opengl_render_frame(pm);
        glBindFramebuffer(GL_FRAMEBUFFER, 0);
        glReadPixels(0, 0, w, h, GL_BGRA, GL_UNSIGNED_BYTE, pixels);
        // GL's first row is the bottom one.
        for (int y = 0; y < h; y++)
            memcpy(flipped + (size_t)y * w * 4, pixels + (size_t)(h - 1 - y) * w * 4, (size_t)w * 4);
        write_all(header, (size_t)hlen);
        write_all(flipped, frame_len);

        next += period;
        double wait = next - now_s();
        if (wait > 0) {
            struct timespec ts = {(time_t)wait, (long)((wait - (time_t)wait) * 1e9)};
            nanosleep(&ts, NULL);
        } else {
            next = now_s(); // fell behind; don't try to catch up
        }
    }
}
