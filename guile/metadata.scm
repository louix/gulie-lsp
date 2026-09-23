;;; Entry script usable from both plain Guile and `guix repl -- FILE ARGS...`.
;;;   guile guile/metadata.scm exports '(srfi srfi-1)'
;;;   guix repl -- guile/metadata.scm service-type '(gnu services ssh)' openssh-service-type
;; (current-filename) is #f when loaded by absolute path, so use the script path.
(add-to-load-path (dirname (car (command-line))))
(use-modules (guile-lsp metadata))
(main (command-line))
