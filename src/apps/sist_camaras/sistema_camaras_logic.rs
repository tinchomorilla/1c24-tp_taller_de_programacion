use std::{
    collections::HashMap,
    io::{Error, ErrorKind},
    sync::{mpsc::Sender, MutexGuard},
};

use crate::{apps::incident_data::incident::Incident, logging::string_logger::StringLogger};

use crate::apps::sist_camaras::{
    camera::Camera,
    types::{hashmap_incs_type::HashmapIncsType, shareable_cameras_type::ShCamerasType},
};

#[derive(Debug)]
pub struct CamerasLogic {
    cameras: ShCamerasType,
    incs_being_managed: HashmapIncsType,
    cameras_tx: Sender<Vec<u8>>,
    logger: StringLogger,
}

impl CamerasLogic {
    /// Crea un struct CamerasLogic con las cámaras pasadas como parámetro e incidentes manejándose vacíos.
    pub fn new(cameras: ShCamerasType, cameras_tx: Sender<Vec<u8>>, logger: StringLogger) -> Self {
        Self {
            cameras,
            incs_being_managed: HashMap::new(),
            cameras_tx,
            logger,
        }
    }

    /// Procesa un Incidente recibido.
    pub fn manage_incident(&mut self, incident: Incident) -> Result<(), Error>{
        // Proceso los incidentes
        if !self.incs_being_managed.contains_key(&incident.get_info()) {
            self.process_first_time_incident(incident)
        } else {
            self.process_known_incident(incident)
        }
    }

    // Aux: (condición "hasta que" del enunciado).
    /// Procesa un incidente cuando un incidente con ese mismo id ya fue recibido anteriormente.
    /// Si su estado es resuelto, vuelve el estado de la/s cámara/s que lo atendían, a ahorro de energía.
    fn process_known_incident(&mut self, inc: Incident) -> Result<(), Error> {
        if inc.is_resolved() {
            self.logger.log(format!(
                "Recibo el inc {} de nuevo, ahora con estado resuelto.",
                inc.get_id()
            ));
            // Busco la/s cámara/s que atendían este incidente
            if let Some(cams_managing_inc) = self.incs_being_managed.get(&inc.get_info()) {
                // sé que existe, por el if de más arriba

                // Cambio el estado de las cámaras que lo manejaban, otra vez a ahorro de energía
                // solamente si el incidente en cuestión era el único que manejaban (si tenía más incidentes en rango, sigue estando activa)
                for camera_id in cams_managing_inc {
                    match self.cameras.lock() {
                        Ok(mut cams) => {
                            if let Some(cam_to_update) = cams.get_mut(camera_id) {
                                self.stop_paying_attention_to(&inc, cam_to_update);
                            }
                        }
                        Err(_) => return Err(Error::new(
                            ErrorKind::Other,
                            "Error al tomar lock en process_first_time_incident.",
                        ))
                    };
                }
            }
            // También elimino la entrada del hashmap que busca por incidente, ya no le doy seguimiento
            self.incs_being_managed.remove(&inc.get_info());
        }
        Ok(())
    }

    /// Elimina el incidente `inc` de la lista de incs a los que la cámara `cam_to_update` estaba prestando atención.
    /// Si eso trajo como consecuencia que la misma volviera a estado `SavingMode` (ie el removido era su último incidente),
    /// entonces envío la cámara para ser publicada por MQTT ya que la misma ha cambiado.
    fn stop_paying_attention_to(&self, inc: &Incident, cam_to_update: &mut Camera) {
        // Actualizo la cámara en cuestión
        let state_has_changed = cam_to_update.remove_from_incs_being_managed(inc.get_info());

        let info = cam_to_update.get_id_and_incs_for_debug_display();
        self.logger
            .log(format!(" la cám queda: cam id y lista de incs: {:?}", info));

        // La envío si cambió de estado
        if state_has_changed {
            self.logger
                .log(format!("Cambiado a SavingMode: {:?}", cam_to_update));
            self.send_camera_bytes(cam_to_update, &self.cameras_tx);
        }
    }

    /// Procesa un incidente cuando el mismo fue recibido por primera vez.
    /// Para cada cámara ve si inc.pos está dentro de alcance de dicha cámara o sus lindantes,
    /// en caso afirmativo, se encarga de lo necesario para que la cámara y sus lindanes cambien su estado a activo.
    fn process_first_time_incident(&mut self, inc: Incident) -> Result<(), Error> {
        if !inc.is_resolved() {
            // inc no resuelto
            match self.cameras.lock() {
                Ok(mut cams) => {
                    println!("Proceso el incidente {:?} por primera vez", inc.get_info());
                    self.logger.log(format!(
                        "Proceso el incidente {:?} por primera vez",
                        inc.get_info()
                    ));
                    let cameras_that_follow_inc =
                        self.get_id_of_cams_that_will_change_state_to_active(&mut cams, &inc);

                    // El vector tiene los ids de todas las cámaras que deben cambiar a activo
                    for cam_id in &cameras_that_follow_inc {
                        if let Some(bordering_cam) = cams.get_mut(cam_id) {
                            self.start_paying_attention_to(&inc, bordering_cam);
                        };
                    }
                    // Y se guarda las cámaras que le dan seguimiento al incidente, para luego poder encontrarlas fácilmente sin recorrer
                    self.incs_being_managed
                        .insert(inc.get_info(), cameras_that_follow_inc);
                }
                Err(_) => {
                    return Err(Error::new(
                        ErrorKind::Other,
                        "Error al tomar lock en process_first_time_incident.",
                    ))
                }
            }
        }
        Ok(())
    }

    /// Devuelve un vector de u8 con los ids de todas las cámaras que darán seguimiento al incidente `inc`.
    fn get_id_of_cams_that_will_change_state_to_active(
        &self,
        cams: &mut MutexGuard<'_, HashMap<u8, Camera>>,
        inc: &Incident,
    ) -> Vec<u8> {
        let mut cameras_that_follow_inc = vec![];

        // Recorremos cada una de las cámaras, para ver si el inc está en su rango
        for (cam_id, camera) in cams.iter_mut() {
            if camera.will_register(inc.get_position()) {
                self.logger
                    .log(format!("En rango de cam: {}, cambiando a Activo.", cam_id));

                // Si sí, se agrega ella
                cameras_that_follow_inc.push(*cam_id);
                // y sus lindantes
                for bordering_cam_id in camera.get_bordering_cams() {
                    cameras_that_follow_inc.push(*bordering_cam_id);
                }

                let info = camera.get_id_and_incs_for_debug_display();
                self.logger
                    .log(format!(" la cám queda: cam id y lista de incs: {:?}", info));
            }
        }
        cameras_that_follow_inc
    }

    /// Agrega el incidente `inc` a la lista de incs a los que la cámara `cam_to_update` presta atención.
    /// Si eso trae como consecuencia que la misma cambiara a estado `Active` (ie el agregado era su primer incidente),
    /// entonces envío la cámara para ser publicada por MQTT ya que la misma ha cambiado.
    fn start_paying_attention_to(&self, inc: &Incident, cam_to_update: &mut Camera) {
        // Agrega el inc a la lista de incs de la cámara, y de sus lindantes, para facilitar que luego puedan volver a su anterior estado
        let state_has_changed = cam_to_update.append_to_incs_being_managed(inc.get_info());

        // La envío si cambió de estado
        if state_has_changed {
            self.logger
                .log(format!("Cambiando a estado Active: {:?}", cam_to_update));
            self.send_camera_bytes(cam_to_update, &self.cameras_tx);
        }
    }

    /// Envía la cámara recibida, por el channel, para que quien la reciba por rx haga el publish.
    /// Además logguea la operación.
    fn send_camera_bytes(&self, camera: &Camera, cameras_tx: &Sender<Vec<u8>>) {
        self.logger
            .log(format!("Sistema-Camaras: envío cámara: {:?}", camera));

        if cameras_tx.send(camera.to_bytes()).is_err() {
            println!("Error al enviar cámara por tx desde hilo abm.");
            self.logger
                .log("Sistema-Camaras: error al enviar cámara por tx desde hilo abm.".to_string());
        }
    }
}
