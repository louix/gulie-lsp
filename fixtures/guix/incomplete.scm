;;; Mid-edit configuration: the user is typing a new service.
(use-modules (gnu) (gnu services ssh))

(operating-system
  (host-name "fixture")
  (services
   (cons* (service openssh-service-type
                   (openssh-configuration
                    (port-nu
