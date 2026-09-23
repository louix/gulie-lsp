;;; Ordinary SRFI-9 records.
(define-module (records)
  #:use-module (srfi srfi-9)
  #:use-module (srfi srfi-9 gnu)
  #:export (<point> make-point point? point-x point-y set-point-y!
            make-segment segment-start segment-end))

(define-record-type <point>
  (make-point x y)
  point?
  (x point-x)
  (y point-y set-point-y!))

(define-immutable-record-type <segment>
  (make-segment start end)
  segment?
  (start segment-start)
  (end segment-end))
