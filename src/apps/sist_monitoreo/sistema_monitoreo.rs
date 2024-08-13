use std::{
    collections::HashMap,
    hash::Hash,
    io::{self, ErrorKind},
    sync::{mpsc, Arc, Mutex},
    thread::{self, JoinHandle},
};

use crossbeam_channel::{unbounded, Receiver as CrossbeamReceiver, Sender as CrossbeamSender};
use std::sync::mpsc::{Receiver as MpscReceiver, Sender as MpscSender};

use crate::{
    apps::{
        apps_mqtt_topics::AppsMqttTopics, common_clients::there_are_no_more_publish_msgs, sist_camaras::camera::Camera, sist_dron::dron_current_info::DronCurrentInfo
    },
    logging::string_logger::StringLogger,
};

use crate::mqtt::{client::mqtt_client::MQTTClient, messages::publish_message::PublishMessage};

use super::ui_sistema_monitoreo::UISistemaMonitoreo;
use crate::apps::{common_clients::exit_when_asked, incident_data::incident::Incident};
use std::fs;
use std::io::Error;

#[derive(Debug)]
pub struct SistemaMonitoreo {
    incidents: Arc<Mutex<Vec<Incident>>>,
    qos: u8,
    logger: StringLogger,
    topics: Vec<(String, u8)>,
    timestamp_by_topic: HashMap<(String, u8), u128>, // ((Topic, id), timestamp)
}

fn leer_qos_desde_archivo(ruta_archivo: &str) -> Result<u8, io::Error> {
    let contenido = fs::read_to_string(ruta_archivo)?;
    let inicio = contenido.find("qos=").ok_or(io::Error::new(
        ErrorKind::NotFound,
        "No se encontró la etiqueta 'qos='",
    ))?;

    let valor_qos = contenido[inicio + 4..].trim().parse::<u8>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            "El valor de QoS no es un número válido",
        )
    })?;
    println!("Valor de QoS: {}", valor_qos);
    Ok(valor_qos)
}

impl SistemaMonitoreo {
    pub fn new(logger: StringLogger) -> Self {
        let qos =
            leer_qos_desde_archivo("src/apps/sist_monitoreo/qos_sistema_monitoreo.properties")
                .unwrap_or(0);
        println!("valor de QoS: {}", qos);
        let topics = vec![
            (AppsMqttTopics::CameraTopic.to_str().to_string(), qos),
            (AppsMqttTopics::DronTopic.to_str().to_string(), qos),
            (AppsMqttTopics::IncidentTopic.to_str().to_string(), qos),
            (AppsMqttTopics::DescTopic.to_str().to_string(), qos),
        ];
        let sistema_monitoreo: SistemaMonitoreo = Self {
            incidents: Arc::new(Mutex::new(Vec::new())), // []
            qos,
            logger,
            topics,
            timestamp_by_topic: HashMap::new(),
        };

        sistema_monitoreo
    }

    pub fn spawn_threads(
        &self,
        publish_message_rx: MpscReceiver<PublishMessage>,
        mqtt_client: MQTTClient,
    ) -> Vec<JoinHandle<()>> {
        let (incident_tx, incident_rx) = mpsc::channel::<Incident>();
        let (exit_tx, exit_rx) = mpsc::channel::<bool>();

        let mut children: Vec<JoinHandle<()>> = vec![];
        let mqtt_client_sh = Arc::new(Mutex::new(mqtt_client));
        let (egui_tx, egui_rx) = unbounded::<PublishMessage>();

        // Exit, cuando ui lo solicite
        children.push(self.spawn_exit_thread(mqtt_client_sh.clone(), exit_rx));

        // Recibe inc de la ui y hace publish
        children.push(self.spawn_publish_incs_thread(mqtt_client_sh.clone(), incident_rx));

        // Recibe msgs por MQTT y los envía para mostrarse en la ui
        children.push(self.spawn_subscribe_to_topics_thread(
            mqtt_client_sh.clone(),
            publish_message_rx,
            egui_tx,
        ));

        // UI
        self.spawn_ui_thread(incident_tx, egui_rx, exit_tx);

        children
    }
    pub fn get_qos(&self) -> u8 {
        self.qos
    }

    fn spawn_ui_thread(
        &self,
        incident_tx: MpscSender<Incident>,
        publish_message_rx: CrossbeamReceiver<PublishMessage>,
        exit_tx: MpscSender<bool>,
    ) {
        // -> JoinHandle<()> {

        // let self_clone = self.clone_ref();
        // thread::spawn(move || {
        if let Err(e) = eframe::run_native(
            "Sistema Monitoreo",
            Default::default(),
            Box::new(|cc| {
                Box::new(UISistemaMonitoreo::new(
                    cc.egui_ctx.clone(),
                    incident_tx,
                    publish_message_rx,
                    exit_tx,
                ))
            }),
        ) {
            self.logger.log(format!("Error en hilo para UI: {:?}.", e));
        }
        // })
        println!("Saliendo de ui.");
        //drop(exit_tx);
    }

    /// Recibe incidente desde la UI, y lo publica por MQTT.
    fn spawn_publish_incs_thread(
        &self,
        mqtt_client: Arc<Mutex<MQTTClient>>,
        rx: MpscReceiver<Incident>,
    ) -> JoinHandle<()> {
        let self_clone = self.clone_ref();
        thread::spawn(move || {
            while let Ok(inc) = rx.recv() {
                self_clone
                    .logger
                    .log(format!("Sistema-Monitoreo: envío incidente: {:?}", inc));
                self_clone.publish_incident(inc, &mqtt_client);
            }
        })
    }

    fn clone_ref(&self) -> Self {
        Self {
            incidents: self.incidents.clone(),
            qos: self.qos,
            logger: self.logger.clone_ref(),
            topics: self.topics.clone(),
            timestamp_by_topic: self.timestamp_by_topic.clone(),
        }
    }

    /// Se suscribe a los topics y queda recibiendo PublishMessages de esos topics.
    /// Delega el procesamiento de cada mensaje recibido por MQTT a otra parte del Sistema Cámaras, enviándolo por un channel.
    fn spawn_subscribe_to_topics_thread(
        &self,
        mqtt_client: Arc<Mutex<MQTTClient>>,
        mqtt_rx: MpscReceiver<PublishMessage>,
        egui_tx: CrossbeamSender<PublishMessage>,
    ) -> JoinHandle<()> {
        let mut self_clone = self.clone_ref();
        thread::spawn(move || {
            self_clone.subscribe_to_topics(mqtt_client);
            if let Err(e) = self_clone.receive_messages_from_subscribed_topics(mqtt_rx, egui_tx) {
                self_clone.logger.log(format!(
                    "Error en hilo para recibir mensajes de MQTT: {:?}.",
                    e
                ));
            }
        })
    }

    /// Se suscribe a los topics.
    fn subscribe_to_topics(&self, mqtt_client: Arc<Mutex<MQTTClient>>) {
        let _ = self.subscribe_to_topics_vec(&mqtt_client);
    }

    fn subscribe_to_topics_vec(&self, mqtt_client: &Arc<Mutex<MQTTClient>>) -> Result<(), Error> {
        if let Ok(mut mqtt_client) = mqtt_client.lock() {
            mqtt_client.mqtt_subscribe(self.topics.clone())?;
            self.logger.log(format!(
                "Sistema-Monitoreo: subscripto a topics {:?}",
                &self.topics
            ));
            Ok(())
        } else {
            Err(Error::new(
                ErrorKind::Other,
                "Error al obtener el lock del mqtt_client",
            ))
        }
    }

    /// Envía a otra parte del sistema de monitoreo, para ser procesado, cada mensaje recibido por MQTT.
    fn receive_messages_from_subscribed_topics(
        &mut self,
        mqtt_rx: MpscReceiver<PublishMessage>,
        egui_tx: CrossbeamSender<PublishMessage>,
    ) -> Result<(), Error>{
        for publish_msg in mqtt_rx {
            self.logger.log(format!(
                "Sistema-Monitoreo: recibió mensaje: {:?}",
                publish_msg
            ));
            //chequeo el timestamp del publish_msg , si es nuevo, lo mando a la ui
            if self.is_newer(&publish_msg)? {
                self.send_publish_message_to_ui(publish_msg, egui_tx.clone())
            }
        }
        
        there_are_no_more_publish_msgs(&self.logger);
        Ok(())
    }

    fn is_newer(&mut self, publish_msg: &PublishMessage) -> Result<bool, Error> {
        let msg_topic = publish_msg.get_topic();

        match AppsMqttTopics::topic_from_str(&msg_topic)? {
            AppsMqttTopics::DronTopic => {
                let current_info = DronCurrentInfo::from_bytes(publish_msg.get_payload())?;
                let id: u8 = current_info.get_id();
                self.update_timestamp_if_newer(msg_topic, id, publish_msg.get_timestamp())?
            }, 
            AppsMqttTopics::CameraTopic => {
                let camera = Camera::from_bytes(&publish_msg.get_payload());
                let id: u8 = camera.get_id();
                self.update_timestamp_if_newer(msg_topic, id, publish_msg.get_timestamp())?
            },
            _ => return Ok(true),
        };
        Ok(true)
    }

    // Función para actualizar el timestamp si el recibido es más nuevo
    fn update_timestamp_if_newer(&mut self, msg_topic: String, id: u8, rcv_timestamp: u128) -> Result<bool, Error> {
        // Genera la clave a partir del topic y el id
        let key = (msg_topic, id);
        // Intenta obtener el último timestamp para la clave dada, o lo inserta si no existe
        let last_timestamp = self.timestamp_by_topic.entry(key).or_insert(rcv_timestamp);
        println!("Timestamp recibido: {}, Timestamp guardado: {} , con ID {} ", rcv_timestamp, *last_timestamp, id);
        
        // Si el timestamp recibido es más nuevo, actualiza el valor y devuelve true
        if rcv_timestamp >= *last_timestamp {
            println!("Se actualiza el timestamp");
            *last_timestamp = rcv_timestamp;
            return Ok(true);
        }
        // Si el mensaje recibido para un mismo ID es más viejo, devuelve false
        return Ok(false);
    }

    fn send_publish_message_to_ui(
        &self,
        msg: PublishMessage,
        egui_tx: CrossbeamSender<PublishMessage>,
    ) {
        let res_send = egui_tx.send(msg);
        match res_send {
            Ok(_) => println!("Enviado mensaje a la UI"),
            Err(e) => println!("Error al enviar mensaje a la UI: {:?}", e),
        }
    }

    /// Hilo para salir desde la UI
    fn spawn_exit_thread(
        &self,
        mqtt_client: Arc<Mutex<MQTTClient>>,
        exit_rx: MpscReceiver<bool>,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            exit_when_asked(mqtt_client, exit_rx);
        })
    }

    fn publish_incident(&self, incident: Incident, mqtt_client: &Arc<Mutex<MQTTClient>>) {
        println!("Sistema-Monitoreo: Publicando incidente.");

        // Hago el publish
        if let Ok(mut mqtt_client) = mqtt_client.lock() {
            let res_publish = mqtt_client.mqtt_publish(
                AppsMqttTopics::IncidentTopic.to_str(),
                &incident.to_bytes(),
                self.get_qos(),
            );
            match res_publish {
                Ok(publish_message) => {
                    self.logger.log(format!(
                        "Sistema-Monitoreo: envío mensaje: {:?}",
                        publish_message
                    ));
                }
                Err(e) => {
                    println!("Sistema-Monitoreo: Error al hacer el publish {:?}", e)
                }
            };
        }
    }

    
}


