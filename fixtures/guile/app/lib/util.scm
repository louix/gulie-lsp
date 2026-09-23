(define-module (app lib util)
  #:use-module (srfi srfi-1)
  #:export (string-length* config-values)
  #:export-syntax (with-timing))

(define (string-length* string)
  "Like string-length, but treat #f as the empty string."
  (if string (string-length string) 0))

(define (config-values config)
  (map cdr config))

(define-syntax with-timing
  (syntax-rules ()
    ((_ label body ...)
     (let ((start (get-internal-real-time)))
       (call-with-values (lambda () body ...)
         (lambda results
           (format (current-error-port) "~a: ~a~%" label
                   (- (get-internal-real-time) start))
           (apply values results)))))))
