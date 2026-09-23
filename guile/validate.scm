;;; Entry script: `guix repl -- guile/validate.scm CONFIG.scm` prints one
;;; validation S-expression. Loads only (guile-lsp validate) from its own
;;; directory; the Guix modules come from the environment it runs in.
(add-to-load-path (dirname (car (command-line))))
(use-modules (guile-lsp validate))
(main (command-line))
