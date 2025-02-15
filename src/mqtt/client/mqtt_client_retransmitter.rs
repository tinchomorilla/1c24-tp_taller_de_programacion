use std::{io::{Error, ErrorKind}, net::Shutdown, sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender}, time::Duration};

use crate::{logging::string_logger::StringLogger, mqtt::{messages::{disconnect_message::DisconnectMessage, message::Message, packet_type::PacketType, publish_message::PublishMessage}, mqtt_utils::utils::write_message_to_stream}};

use super::{ack_message::ACKMessage, mqtt_client::ClientStreamType};

/// Parte interna de `MQTTClient` encargada de manejar los ack y las retransmisiones.
/// Conserva el extramo receptor de un channel (`ack_rx`).
#[derive(Debug)]
pub struct Retransmitter {
    ack_rx: Receiver<ACKMessage>,
    stream: ClientStreamType,
    logger: StringLogger,
}

impl Retransmitter {
    /// Crea y devuelve un Retransmitter, encargado del envío y las retransmisiones, y el extremo de envío de un channel.
    pub fn new(stream: ClientStreamType, logger: StringLogger) -> (Self, Sender<ACKMessage>) {
        let (ack_tx, ack_rx) = channel::<ACKMessage>();
        (Self { ack_rx , stream , logger }, ack_tx)
    }
    
    /// Envía el mensaje `msg` recibido una vez, espera por el ack, y si es necesario lo retransmite una cierta
    /// cantidad de veces.
    pub fn send_and_retransmit<T: Message>(&mut self, msg: &T) -> Result<(), Error> {
        self.logger.log("Mqtt: Enviando msg.".to_string());
        self.send_msg(msg.to_bytes())?;
        if let Err(e) = self.wait_for_ack_and_retransmit(msg) {
            println!("Error al esperar ack: {:?}", e);
            self.logger.log(format!("Error al esperar ack: {:?}", e));
        };
        self.logger.log("Mqtt: recibido ack.".to_string());
        Ok(())
    }

    /// Espera por el ack y si no lo recibe retransmite, teniendo en cuenta el tipo de paquete,
    /// para el publish considera su nivel de qos.
    fn wait_for_ack_and_retransmit<T: Message>(&mut self, msg: &T) -> Result<(), Error> {
        match msg.get_type() {
            // Si es publish, ver el qos
            PacketType::Publish => {
                if let Some(pub_msg) = msg.as_any().downcast_ref::<PublishMessage>() {
                    let qos = pub_msg.get_qos();
                    if qos == 1 {
                        return self.wait_and_retransmit(pub_msg);
                    } else {
                        return Ok(());
                    }
                }
            }
            PacketType::Subscribe => {
                return self.wait_and_retransmit(msg);
            }
            _ => {}
        }

        Ok(())
    }

    /// Espera a recibir el ack para el packet_id del mensaje `msg`, si no lo recibe, retransmite.
    fn wait_and_retransmit<T: Message>(&mut self, msg: &T) -> Result<(), Error> {
        let packet_id = msg.get_packet_id();
        // Espero la primera vez, para el publish que hicimos arriba. Si se recibió ack, no hay que hacer nada más.
        let mut received_ack = self.has_ack_arrived(packet_id)?;
        if received_ack {
            return Ok(());
        }

        // No recibí ack, entonces tengo que continuar retransmitiendo, hasta un máx de veces.
        const AMOUNT_OF_RETRIES: u8 = 5; // cant de veces que va a reintentar, hasta que desista y dé error.
        let mut remaining_retries = AMOUNT_OF_RETRIES;

        while !received_ack && remaining_retries > 0 {
            // Lo vuelvo a enviar, y a verificar si llega el ack.
            
            self.send_msg(msg.to_bytes())?;
            received_ack = self.has_ack_arrived(packet_id)?;
            self.logger.log("Mqtt: Retransmitiendo...".to_string());

            remaining_retries -= 1;
        }

        if !received_ack {
            // Ya salí del while, retransmití muchas veces y nunca recibí el ack, desisto.
            return Err(Error::new(
                ErrorKind::Other,
                "MAXRETRIES, se retransmitió sin éxito.",
            ));
        }

        Ok(())
    }

    /// Espera a que MQTTListener le informe por este rx que llegó el ack. En ese caso devuelve ok.
    /// Si eso no ocurre, debe retransmitir el mensaje original (el msg cuyo ack está esperando)
    /// hasta que llegue su ack o bien se llegue a una cantidad máxima de intentos definida como constante.
    /// Devuelve si recibió el ack.
    fn has_ack_arrived(&self, packet_id: Option<u16>) -> Result<bool, Error> {
        // Extrae el packet_id
        if let Some(packet_id) = packet_id {
            self.start_waiting_and_check_for_ack(packet_id)
        } else {
                Err(Error::new(
                ErrorKind::Other,
                "No se pudo obtener el packet id del mensaje publish",
            ))
        }
    }

    /// Espera por el ack como máximo un cierto tiempo,
    /// si no se cerró la conexión con listener, devuelve Ok de si llega el ack.
    fn start_waiting_and_check_for_ack(&self, packet_id: u16) -> Result<bool, Error> {
        // Leo esperando un cierto tiempo, si en el período [0, ese tiempo) no me llega el ack, lo quiero retransmitir.
        const ACK_WAITING_INTERVAL: u64 = 1000;
        match self.ack_rx.recv_timeout(Duration::from_millis(ACK_WAITING_INTERVAL)){
            Ok(ack_message) => {
                // Se recibió el ack
                if let Some(packet_identifier) = ack_message.get_packet_id() {
                    if packet_id == packet_identifier {
                        println!("   llegó el ack {:?}", ack_message); 
                        return Ok(true);
                    }
                }
            },
            Err(e) => {
                match e {
                    RecvTimeoutError::Timeout => {
                        // Se cumplió el tiempo y el ack No se recibió.
                        return Ok(false);

                    },
                    RecvTimeoutError::Disconnected => {
                        // Se cerró el channel. Termina el programa.
                        // Ver.
                    },
                }
            },
        }
        Ok(false)
    }

    /// Función para ser usada por `MQTTClient`, cuando el `Retransmitter` haya determinado que el `msg` debe
    /// enviarse por el stream a server.
    fn send_msg(&mut self, bytes_msg: Vec<u8>) -> Result<(), Error> {
        write_message_to_stream(&bytes_msg, &mut self.stream)?;
        Ok(())
    }
    
    /// Envía el mensaje disconnect recibido por parámetro y cierra la conexión.
    pub fn send_and_shutdown_stream(&mut self, msg: DisconnectMessage) -> Result<(), Error> {
        self.send_msg(msg.to_bytes())?;
        // Cerramos la conexión con el servidor
        self.stream.shutdown(Shutdown::Both)?;
        self.logger.log("Mqtt: Conexión cerrada.".to_string());

        Ok(())
    }

}