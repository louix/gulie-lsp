;;; Entry script usable from both plain Guile and `guix repl -- FILE ARGS...`.
;; (current-filename) is #f when loaded by absolute path, so use the script path.
(add-to-load-path (dirname (car (command-line))))
(use-modules (guile-lsp worker))
(main (command-line))
