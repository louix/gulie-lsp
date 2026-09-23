;;; Multi-module application entry point with renamed and prefixed imports.
(define-module (app main)
  #:use-module ((app lib util) #:prefix util:)
  #:use-module ((app lib config) #:select (load-config) #:renamer (symbol-prefix-proc 'cfg-))
  #:use-module ((srfi srfi-1) #:select ((fold . reduce-left)))
  #:export (run))

(define (run)
  "Run the application and return its exit status."
  (let ((config (cfg-load-config "app.conf")))
    (reduce-left + 0 (map util:string-length* (util:config-values config)))))
