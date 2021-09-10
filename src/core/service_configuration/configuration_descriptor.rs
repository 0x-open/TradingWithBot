use std::hash::Hash;

#[derive(Hash, Debug, Clone, Eq, PartialEq)]
pub struct ConfigurationDescriptor {
    pub service_name: String,
    pub service_configuration_key: String,
}

impl ConfigurationDescriptor {
    pub fn new(service_name: String, service_configuration_key: String) -> Self {
        Self {
            service_name,
            service_configuration_key,
        }
    }
}
