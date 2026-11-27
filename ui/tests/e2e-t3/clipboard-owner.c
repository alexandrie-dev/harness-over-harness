#include <gtk/gtk.h>
#include <stdio.h>

enum { HTML, PNG };

static void provide(GtkClipboard *clipboard, GtkSelectionData *selection,
                    guint format, gpointer fixture)
{
    (void)clipboard;
    static const guchar html[] = "<img alt=\"\">";
    gsize length = sizeof(html) - 1;
    const guchar *bytes = html;
    if (format == PNG)
        bytes = g_bytes_get_data(fixture, &length);
    gtk_selection_data_set(selection, gtk_selection_data_get_target(selection),
                           8, bytes, (gint)length);
}

int main(int argc, char **argv)
{
    gtk_init(&argc, &argv);
    gchar *png;
    gsize size;
    if (argc != 2 || !g_file_get_contents(argv[1], &png, &size, NULL))
        return 1;

    GtkTargetEntry formats[] = {{"text/html", 0, HTML}, {"image/png", 0, PNG}};
    GtkClipboard *clipboard = gtk_clipboard_get(GDK_SELECTION_CLIPBOARD);
    GBytes *fixture = g_bytes_new_take(png, size);
    if (!gtk_clipboard_set_with_data(clipboard, formats, G_N_ELEMENTS(formats),
                                    provide, NULL, fixture))
        return 2;

    puts("clipboard-ready");
    fflush(stdout);
    gtk_main();
    return 0;
}
