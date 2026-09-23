;;; Imports a module that does not exist anywhere on the load path.
(define-module (pathological missing-import)
  #:use-module (does not exist))
(define x 1)
