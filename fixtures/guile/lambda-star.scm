;;; Optional, keyword, and rest arguments.
(define-module (lambda-star)
  #:use-module (ice-9 curried-definitions)
  #:export (connect send-all curried))

(define* (connect host #:optional (port 80) #:key (timeout 30) tls? #:rest extra)
  "Connect to HOST on PORT.  TIMEOUT is in seconds."
  (list host port timeout tls? extra))

(define (send-all socket . messages)
  (for-each (lambda (m) (display m socket)) messages))

(define ((curried a) b)
  (+ a b))
