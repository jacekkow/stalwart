/*
 * SPDX-FileCopyrightText: 2020 Stalwart Labs LLC <hello@stalw.art>
 *
 * SPDX-License-Identifier: AGPL-3.0-only OR LicenseRef-SEL
 */

use std::{collections::hash_map::Entry, fmt, str::FromStr};

use ahash::AHashSet;
use nlp::tokenizers::word::WordTokenizer;
use serde::{
    Deserializer, Serializer,
    de::{self, IgnoredAny, Visitor},
    ser::SerializeMap,
};
use store::{
    U64_LEN,
    backend::MAX_TOKEN_LENGTH,
    write::{BatchBuilder, DirectoryClass},
};

use crate::{
    ArchivedPrincipal, Permission, PermissionGrant, Principal, PrincipalData, ROLE_ADMIN, Type,
    backend::internal::{PrincipalField, PrincipalSet, PrincipalUpdate, PrincipalValue},
};

impl Principal {
    pub fn new(id: u32, typ: Type) -> Self {
        Self {
            id,
            typ,
            name: "".into(),
            description: None,
            secrets: Default::default(),
            emails: Default::default(),
            quota: Default::default(),
            tenant: Default::default(),
            data: Default::default(),
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn typ(&self) -> Type {
        self.typ
    }

    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    pub fn quota(&self) -> u64 {
        self.quota.unwrap_or_default()
    }

    pub fn principal_quota(&self, typ: &Type) -> Option<u64> {
        self.data
            .iter()
            .find_map(|d| {
                if let PrincipalData::PrincipalQuota(q) = d {
                    Some(q)
                } else {
                    None
                }
            })
            .and_then(|quotas| {
                quotas
                    .iter()
                    .find_map(|q| if q.typ == *typ { Some(q.quota) } else { None })
            })
    }


    #[cfg(not(feature = "enterprise"))]
    pub fn tenant(&self) -> Option<u32> {
        None
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    pub fn member_of(&self) -> &[u32] {
        self.data
            .iter()
            .find_map(|item| {
                if let PrincipalData::MemberOf(items) = item {
                    items.as_slice().into()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn member_of_mut(&mut self) -> Option<&mut Vec<u32>> {
        self.data.iter_mut().find_map(|item| {
            if let PrincipalData::MemberOf(items) = item {
                items.into()
            } else {
                None
            }
        })
    }

    pub fn roles(&self) -> &[u32] {
        self.data
            .iter()
            .find_map(|item| {
                if let PrincipalData::Roles(items) = item {
                    items.as_slice().into()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn permissions(&self) -> &[PermissionGrant] {
        self.data
            .iter()
            .find_map(|item| {
                if let PrincipalData::Permissions(items) = item {
                    items.as_slice().into()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn urls(&self) -> &[String] {
        self.data
            .iter()
            .find_map(|item| {
                if let PrincipalData::Urls(items) = item {
                    items.as_slice().into()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn roles_mut(&mut self) -> Option<&mut Vec<u32>> {
        self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Roles(items) = item {
                items.into()
            } else {
                None
            }
        })
    }

    pub fn lists(&self) -> &[u32] {
        self.data
            .iter()
            .find_map(|item| {
                if let PrincipalData::Lists(items) = item {
                    items.as_slice().into()
                } else {
                    None
                }
            })
            .unwrap_or_default()
    }

    pub fn picture(&self) -> Option<&String> {
        self.data.iter().find_map(|item| {
            if let PrincipalData::Picture(picture) = item {
                picture.into()
            } else {
                None
            }
        })
    }

    pub fn picture_mut(&mut self) -> Option<&mut String> {
        self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Picture(picture) = item {
                picture.into()
            } else {
                None
            }
        })
    }

    pub fn add_permission(&mut self, permission: Permission, grant: bool) {
        if let Some(permissions) = self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Permissions(permissions) = item {
                Some(permissions)
            } else {
                None
            }
        }) {
            if let Some(current) = permissions.iter_mut().find(|p| p.permission == permission) {
                current.grant = grant;
            } else {
                permissions.push(PermissionGrant { permission, grant });
            }
        } else {
            self.data
                .push(PrincipalData::Permissions(vec![PermissionGrant {
                    permission,
                    grant,
                }]));
        }
    }

    pub fn add_permissions(&mut self, iter: impl Iterator<Item = PermissionGrant>) {
        if let Some(permissions) = self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Permissions(permissions) = item {
                Some(permissions)
            } else {
                None
            }
        }) {
            permissions.extend(iter);
        } else {
            self.data.push(PrincipalData::Permissions(iter.collect()));
        }
    }

    pub fn remove_permission(&mut self, permission: Permission, grant: bool) {
        if let Some(permissions) = self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Permissions(permissions) = item {
                Some(permissions)
            } else {
                None
            }
        }) {
            if let Some(idx) = permissions
                .iter_mut()
                .position(|p| p.permission == permission && p.grant == grant)
            {
                permissions.swap_remove(idx);
            }
        }
    }

    pub fn remove_permissions(&mut self, grant: bool) {
        if let Some(permissions) = self.data.iter_mut().find_map(|item| {
            if let PrincipalData::Permissions(permissions) = item {
                Some(permissions)
            } else {
                None
            }
        }) {
            permissions.retain(|p| p.grant != grant);
        }
    }

    pub fn update_external(&mut self, mut external: Principal) -> Vec<PrincipalUpdate> {
        let mut updates = Vec::new();

        // Add external members
        if let Some(member_of) = external.member_of_mut().filter(|s| !s.is_empty()) {
            self.data
                .push(PrincipalData::MemberOf(std::mem::take(member_of)));
        }

        // If the principal has no roles, take the ones from the external principal
        if let Some(roles) = external.roles_mut().filter(|s| !s.is_empty()) {
            if self.roles().is_empty() {
                self.data.push(PrincipalData::Roles(std::mem::take(roles)));
            }
        }

        if external.description.as_ref().is_some_and(|v| !v.is_empty())
            && self.description != external.description
        {
            self.description = external.description;
            updates.push(PrincipalUpdate::set(
                PrincipalField::Description,
                PrincipalValue::String(self.description.clone().unwrap()),
            ));
        }

        for (name, field, external_field) in [
            (PrincipalField::Secrets, &mut self.secrets, external.secrets),
            (PrincipalField::Emails, &mut self.emails, external.emails),
        ] {
            if !external_field.is_empty() && &external_field != field {
                *field = external_field;
                updates.push(PrincipalUpdate::set(
                    name,
                    PrincipalValue::StringList(field.clone()),
                ));
            }
        }

        if external.quota.is_some() && self.quota != external.quota {
            self.quota = external.quota;
            updates.push(PrincipalUpdate::set(
                PrincipalField::Quota,
                PrincipalValue::Integer(self.quota.unwrap()),
            ));
        }

        updates
    }

    pub fn fallback_admin(fallback_pass: impl Into<String>) -> Self {
        Principal {
            id: u32::MAX,
            typ: Type::Individual,
            name: "Fallback Administrator".into(),
            secrets: vec![fallback_pass.into()],
            data: vec![PrincipalData::Roles(vec![ROLE_ADMIN])],
            description: Default::default(),
            emails: Default::default(),
            quota: Default::default(),
            tenant: Default::default(),
        }
    }
}

impl PrincipalSet {
    pub fn new(id: u32, typ: Type) -> Self {
        Self {
            id,
            typ,
            ..Default::default()
        }
    }

    pub fn id(&self) -> u32 {
        self.id
    }

    pub fn typ(&self) -> Type {
        self.typ
    }

    pub fn name(&self) -> &str {
        self.get_str(PrincipalField::Name).unwrap_or_default()
    }

    pub fn has_name(&self) -> bool {
        self.fields.contains_key(&PrincipalField::Name)
    }

    pub fn quota(&self) -> u64 {
        self.get_int(PrincipalField::Quota).unwrap_or_default()
    }


    #[cfg(not(feature = "enterprise"))]
    pub fn tenant(&self) -> Option<u32> {
        None
    }

    pub fn description(&self) -> Option<&str> {
        self.get_str(PrincipalField::Description)
    }

    pub fn get_str(&self, key: PrincipalField) -> Option<&str> {
        self.fields.get(&key).and_then(|v| v.as_str())
    }

    pub fn get_int(&self, key: PrincipalField) -> Option<u64> {
        self.fields.get(&key).and_then(|v| v.as_int())
    }

    pub fn get_str_array(&self, key: PrincipalField) -> Option<&[String]> {
        self.fields.get(&key).and_then(|v| match v {
            PrincipalValue::StringList(v) => Some(v.as_slice()),
            PrincipalValue::String(v) => Some(std::slice::from_ref(v)),
            PrincipalValue::Integer(_) | PrincipalValue::IntegerList(_) => None,
        })
    }

    pub fn get_int_array(&self, key: PrincipalField) -> Option<&[u64]> {
        self.fields.get(&key).and_then(|v| match v {
            PrincipalValue::IntegerList(v) => Some(v.as_slice()),
            PrincipalValue::Integer(v) => Some(std::slice::from_ref(v)),
            PrincipalValue::String(_) | PrincipalValue::StringList(_) => None,
        })
    }

    pub fn take(&mut self, key: PrincipalField) -> Option<PrincipalValue> {
        self.fields.remove(&key)
    }

    pub fn take_str(&mut self, key: PrincipalField) -> Option<String> {
        self.take(key).and_then(|v| match v {
            PrincipalValue::String(s) => Some(s),
            PrincipalValue::StringList(l) => l.into_iter().next(),
            PrincipalValue::Integer(i) => Some(i.to_string()),
            PrincipalValue::IntegerList(l) => l.into_iter().next().map(|i| i.to_string()),
        })
    }

    pub fn take_int(&mut self, key: PrincipalField) -> Option<u64> {
        self.take(key).and_then(|v| match v {
            PrincipalValue::Integer(i) => Some(i),
            PrincipalValue::IntegerList(l) => l.into_iter().next(),
            PrincipalValue::String(s) => s.parse().ok(),
            PrincipalValue::StringList(l) => l.into_iter().next().and_then(|s| s.parse().ok()),
        })
    }

    pub fn take_str_array(&mut self, key: PrincipalField) -> Option<Vec<String>> {
        self.take(key).map(|v| v.into_str_array())
    }

    pub fn take_int_array(&mut self, key: PrincipalField) -> Option<Vec<u64>> {
        self.take(key).map(|v| v.into_int_array())
    }

    pub fn iter_str(
        &self,
        key: PrincipalField,
    ) -> Box<dyn Iterator<Item = &String> + Sync + Send + '_> {
        self.fields
            .get(&key)
            .map(|v| v.iter_str())
            .unwrap_or_else(|| Box::new(std::iter::empty()))
    }

    pub fn iter_mut_str(
        &mut self,
        key: PrincipalField,
    ) -> Box<dyn Iterator<Item = &mut String> + Sync + Send + '_> {
        self.fields
            .get_mut(&key)
            .map(|v| v.iter_mut_str())
            .unwrap_or_else(|| Box::new(std::iter::empty()))
    }

    pub fn iter_int(
        &self,
        key: PrincipalField,
    ) -> Box<dyn Iterator<Item = u64> + Sync + Send + '_> {
        self.fields
            .get(&key)
            .map(|v| v.iter_int())
            .unwrap_or_else(|| Box::new(std::iter::empty()))
    }

    pub fn iter_mut_int(
        &mut self,
        key: PrincipalField,
    ) -> Box<dyn Iterator<Item = &mut u64> + Sync + Send + '_> {
        self.fields
            .get_mut(&key)
            .map(|v| v.iter_mut_int())
            .unwrap_or_else(|| Box::new(std::iter::empty()))
    }

    pub fn append_int(&mut self, key: PrincipalField, value: impl Into<u64>) -> &mut Self {
        let value = value.into();
        match self.fields.entry(key) {
            Entry::Occupied(v) => {
                let v = v.into_mut();

                match v {
                    PrincipalValue::IntegerList(v) => {
                        if !v.contains(&value) {
                            v.push(value);
                        }
                    }
                    PrincipalValue::Integer(i) => {
                        if value != *i {
                            *v = PrincipalValue::IntegerList(vec![*i, value]);
                        }
                    }
                    PrincipalValue::String(s) => {
                        *v =
                            PrincipalValue::IntegerList(vec![s.parse().unwrap_or_default(), value]);
                    }
                    PrincipalValue::StringList(l) => {
                        *v = PrincipalValue::IntegerList(
                            l.iter()
                                .map(|s| s.parse().unwrap_or_default())
                                .chain(std::iter::once(value))
                                .collect(),
                        );
                    }
                }
            }
            Entry::Vacant(v) => {
                v.insert(PrincipalValue::IntegerList(vec![value]));
            }
        }

        self
    }

    pub fn append_str(&mut self, key: PrincipalField, value: impl Into<String>) -> &mut Self {
        let value = value.into();
        match self.fields.entry(key) {
            Entry::Occupied(v) => {
                let v = v.into_mut();

                match v {
                    PrincipalValue::StringList(v) => {
                        if !v.contains(&value) {
                            v.push(value);
                        }
                    }
                    PrincipalValue::String(s) => {
                        if s != &value {
                            *v = PrincipalValue::StringList(vec![std::mem::take(s), value]);
                        }
                    }
                    PrincipalValue::Integer(i) => {
                        *v = PrincipalValue::StringList(vec![i.to_string(), value]);
                    }
                    PrincipalValue::IntegerList(l) => {
                        *v = PrincipalValue::StringList(
                            l.iter()
                                .map(|i| i.to_string())
                                .chain(std::iter::once(value))
                                .collect(),
                        );
                    }
                }
            }
            Entry::Vacant(v) => {
                v.insert(PrincipalValue::StringList(vec![value]));
            }
        }
        self
    }

    pub fn prepend_str(&mut self, key: PrincipalField, value: impl Into<String>) -> &mut Self {
        let value = value.into();
        match self.fields.entry(key) {
            Entry::Occupied(v) => {
                let v = v.into_mut();

                match v {
                    PrincipalValue::StringList(v) => {
                        if !v.contains(&value) {
                            v.insert(0, value);
                        }
                    }
                    PrincipalValue::String(s) => {
                        if s != &value {
                            *v = PrincipalValue::StringList(vec![value, std::mem::take(s)]);
                        }
                    }
                    PrincipalValue::Integer(i) => {
                        *v = PrincipalValue::StringList(vec![value, i.to_string()]);
                    }
                    PrincipalValue::IntegerList(l) => {
                        *v = PrincipalValue::StringList(
                            std::iter::once(value)
                                .chain(l.iter().map(|i| i.to_string()))
                                .collect(),
                        );
                    }
                }
            }
            Entry::Vacant(v) => {
                v.insert(PrincipalValue::StringList(vec![value]));
            }
        }
        self
    }

    pub fn set(&mut self, key: PrincipalField, value: impl Into<PrincipalValue>) -> &mut Self {
        self.fields.insert(key, value.into());
        self
    }

    pub fn with_field(mut self, key: PrincipalField, value: impl Into<PrincipalValue>) -> Self {
        self.set(key, value);
        self
    }

    pub fn with_opt_field(
        mut self,
        key: PrincipalField,
        value: Option<impl Into<PrincipalValue>>,
    ) -> Self {
        if let Some(value) = value {
            self.set(key, value);
        }
        self
    }

    pub fn has_field(&self, key: PrincipalField) -> bool {
        self.fields.contains_key(&key)
    }

    pub fn has_str_value(&self, key: PrincipalField, value: &str) -> bool {
        self.fields.get(&key).is_some_and(|v| match v {
            PrincipalValue::String(v) => v == value,
            PrincipalValue::StringList(l) => l.iter().any(|v| v == value),
            PrincipalValue::Integer(_) | PrincipalValue::IntegerList(_) => false,
        })
    }

    pub fn has_int_value(&self, key: PrincipalField, value: u64) -> bool {
        self.fields.get(&key).is_some_and(|v| match v {
            PrincipalValue::Integer(v) => *v == value,
            PrincipalValue::IntegerList(l) => l.contains(&value),
            PrincipalValue::String(_) | PrincipalValue::StringList(_) => false,
        })
    }

    pub fn find_str(&self, value: &str) -> bool {
        self.fields.values().any(|v| v.find_str(value))
    }

    pub fn field_len(&self, key: PrincipalField) -> usize {
        self.fields.get(&key).map_or(0, |v| match v {
            PrincipalValue::String(_) => 1,
            PrincipalValue::StringList(l) => l.len(),
            PrincipalValue::Integer(_) => 1,
            PrincipalValue::IntegerList(l) => l.len(),
        })
    }

    pub fn remove(&mut self, key: PrincipalField) -> Option<PrincipalValue> {
        self.fields.remove(&key)
    }

    pub fn retain_str<F>(&mut self, key: PrincipalField, mut f: F)
    where
        F: FnMut(&String) -> bool,
    {
        if let Some(value) = self.fields.get_mut(&key) {
            match value {
                PrincipalValue::String(s) => {
                    if !f(s) {
                        self.fields.remove(&key);
                    }
                }
                PrincipalValue::StringList(l) => {
                    l.retain(f);
                    if l.is_empty() {
                        self.fields.remove(&key);
                    }
                }
                _ => {}
            }
        }
    }

    pub fn retain_int<F>(&mut self, key: PrincipalField, mut f: F)
    where
        F: FnMut(&u64) -> bool,
    {
        if let Some(value) = self.fields.get_mut(&key) {
            match value {
                PrincipalValue::Integer(i) => {
                    if !f(i) {
                        self.fields.remove(&key);
                    }
                }
                PrincipalValue::IntegerList(l) => {
                    l.retain(f);
                    if l.is_empty() {
                        self.fields.remove(&key);
                    }
                }
                _ => {}
            }
        }
    }
}

impl PrincipalValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            PrincipalValue::String(v) => Some(v.as_str()),
            PrincipalValue::StringList(v) => v.first().map(|s| s.as_str()),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<u64> {
        match self {
            PrincipalValue::Integer(v) => Some(*v),
            PrincipalValue::IntegerList(v) => v.first().copied(),
            _ => None,
        }
    }

    pub fn iter_str(&self) -> Box<dyn Iterator<Item = &String> + Sync + Send + '_> {
        match self {
            PrincipalValue::String(v) => Box::new(std::iter::once(v)),
            PrincipalValue::StringList(v) => Box::new(v.iter()),
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn iter_mut_str(&mut self) -> Box<dyn Iterator<Item = &mut String> + Sync + Send + '_> {
        match self {
            PrincipalValue::String(v) => Box::new(std::iter::once(v)),
            PrincipalValue::StringList(v) => Box::new(v.iter_mut()),
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn iter_int(&self) -> Box<dyn Iterator<Item = u64> + Sync + Send + '_> {
        match self {
            PrincipalValue::Integer(v) => Box::new(std::iter::once(*v)),
            PrincipalValue::IntegerList(v) => Box::new(v.iter().copied()),
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn iter_mut_int(&mut self) -> Box<dyn Iterator<Item = &mut u64> + Sync + Send + '_> {
        match self {
            PrincipalValue::Integer(v) => Box::new(std::iter::once(v)),
            PrincipalValue::IntegerList(v) => Box::new(v.iter_mut()),
            _ => Box::new(std::iter::empty()),
        }
    }

    pub fn into_array(self) -> Self {
        match self {
            PrincipalValue::String(v) => PrincipalValue::StringList(vec![v]),
            PrincipalValue::Integer(v) => PrincipalValue::IntegerList(vec![v]),
            v => v,
        }
    }

    pub fn into_str_array(self) -> Vec<String> {
        match self {
            PrincipalValue::StringList(v) => v,
            PrincipalValue::String(v) => vec![v],
            PrincipalValue::Integer(v) => vec![v.to_string()],
            PrincipalValue::IntegerList(v) => v.into_iter().map(|v| v.to_string()).collect(),
        }
    }

    pub fn into_int_array(self) -> Vec<u64> {
        match self {
            PrincipalValue::IntegerList(v) => v,
            PrincipalValue::Integer(v) => vec![v],
            PrincipalValue::String(v) => vec![v.parse().unwrap_or_default()],
            PrincipalValue::StringList(v) => v
                .into_iter()
                .map(|v| v.parse().unwrap_or_default())
                .collect(),
        }
    }

    pub fn serialized_size(&self) -> usize {
        match self {
            PrincipalValue::String(s) => s.len() + 2,
            PrincipalValue::StringList(s) => s.iter().map(|s| s.len() + 2).sum(),
            PrincipalValue::Integer(_) => U64_LEN,
            PrincipalValue::IntegerList(l) => l.len() * U64_LEN,
        }
    }

    pub fn find_str(&self, value: &str) -> bool {
        match self {
            PrincipalValue::String(s) => s.to_lowercase().contains(value),
            PrincipalValue::StringList(l) => l.iter().any(|s| s.to_lowercase().contains(value)),
            _ => false,
        }
    }
}

impl From<u64> for PrincipalValue {
    fn from(v: u64) -> Self {
        Self::Integer(v)
    }
}

impl From<String> for PrincipalValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&str> for PrincipalValue {
    fn from(v: &str) -> Self {
        Self::String(v.into())
    }
}

impl From<Vec<String>> for PrincipalValue {
    fn from(v: Vec<String>) -> Self {
        Self::StringList(v)
    }
}

impl From<Vec<u64>> for PrincipalValue {
    fn from(v: Vec<u64>) -> Self {
        Self::IntegerList(v)
    }
}

impl From<u32> for PrincipalValue {
    fn from(v: u32) -> Self {
        Self::Integer(v as u64)
    }
}

impl From<Vec<u32>> for PrincipalValue {
    fn from(v: Vec<u32>) -> Self {
        Self::IntegerList(v.into_iter().map(|v| v as u64).collect())
    }
}

pub(crate) fn build_search_index(
    batch: &mut BatchBuilder,
    principal_id: u32,
    current: Option<&ArchivedPrincipal>,
    new: Option<&Principal>,
) {
    let mut current_words = AHashSet::new();
    let mut new_words = AHashSet::new();

    if let Some(current) = current {
        for word in [Some(current.name.as_str()), current.description.as_deref()]
            .into_iter()
            .chain(current.emails.iter().map(|s| Some(s.as_str())))
            .flatten()
        {
            current_words.extend(WordTokenizer::new(word, MAX_TOKEN_LENGTH).map(|t| t.word));
        }
    }

    if let Some(new) = new {
        for word in [Some(new.name.as_str()), new.description.as_deref()]
            .into_iter()
            .chain(new.emails.iter().map(|s| Some(s.as_str())))
            .flatten()
        {
            new_words.extend(WordTokenizer::new(word, MAX_TOKEN_LENGTH).map(|t| t.word));
        }
    }

    for word in new_words.difference(&current_words) {
        batch.set(
            DirectoryClass::Index {
                word: word.as_bytes().to_vec(),
                principal_id,
            },
            vec![],
        );
    }

    for word in current_words.difference(&new_words) {
        batch.clear(DirectoryClass::Index {
            word: word.as_bytes().to_vec(),
            principal_id,
        });
    }
}

impl Type {
    pub fn to_jmap(&self) -> &'static str {
        match self {
            Self::Individual => "individual",
            Self::Group => "group",
            Self::Resource => "resource",
            Self::Location => "location",
            Self::Other => "other",
            Self::List => "list",
            Self::Tenant => "tenant",
            Self::Role => "role",
            Self::Domain => "domain",
            Self::ApiKey => "apiKey",
            Self::OauthClient => "oauthClient",
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Individual => "Individual",
            Self::Group => "Group",
            Self::Resource => "Resource",
            Self::Location => "Location",
            Self::Tenant => "Tenant",
            Self::List => "List",
            Self::Other => "Other",
            Self::Role => "Role",
            Self::Domain => "Domain",
            Self::ApiKey => "API Key",
            Self::OauthClient => "OAuth Client",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "individual" => Some(Type::Individual),
            "group" => Some(Type::Group),
            "resource" => Some(Type::Resource),
            "location" => Some(Type::Location),
            "list" => Some(Type::List),
            "tenant" => Some(Type::Tenant),
            "superuser" => Some(Type::Individual), // legacy
            "role" => Some(Type::Role),
            "domain" => Some(Type::Domain),
            "apiKey" => Some(Type::ApiKey),
            "oauthClient" => Some(Type::OauthClient),
            _ => None,
        }
    }

    pub const MAX_ID: usize = 11;

    pub fn from_u8(value: u8) -> Self {
        match value {
            0 => Type::Individual,
            1 => Type::Group,
            2 => Type::Resource,
            3 => Type::Location,
            4 => Type::Other, // legacy
            5 => Type::List,
            6 => Type::Other,
            7 => Type::Domain,
            8 => Type::Tenant,
            9 => Type::Role,
            10 => Type::ApiKey,
            11 => Type::OauthClient,
            _ => Type::Other,
        }
    }
}

impl FromStr for Type {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Type::parse(s).ok_or(())
    }
}

impl serde::Serialize for PrincipalSet {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut map = serializer.serialize_map(None)?;

        map.serialize_entry("id", &self.id)?;
        map.serialize_entry("type", &self.typ.to_jmap())?;

        for (key, value) in &self.fields {
            match value {
                PrincipalValue::String(v) => map.serialize_entry(key.as_str(), v)?,
                PrincipalValue::StringList(v) => map.serialize_entry(key.as_str(), v)?,
                PrincipalValue::Integer(v) => map.serialize_entry(key.as_str(), v)?,
                PrincipalValue::IntegerList(v) => map.serialize_entry(key.as_str(), v)?,
            };
        }

        map.end()
    }
}

const MAX_STRING_LEN: usize = 512;

impl<'de> serde::Deserialize<'de> for PrincipalValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PrincipalValueVisitor;

        impl<'de> Visitor<'de> for PrincipalValueVisitor {
            type Value = PrincipalValue;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("an optional values or a sequence of values")
            }

            fn visit_none<E>(self) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(PrincipalValue::String("".into()))
            }

            fn visit_some<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(PrincipalValue::Integer(value))
            }

            fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() <= MAX_STRING_LEN {
                    Ok(PrincipalValue::String(value))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() <= MAX_STRING_LEN {
                    Ok(PrincipalValue::String(value.into()))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut vec_u64 = Vec::new();
                let mut vec_string = Vec::new();

                while let Some(value) = seq.next_element::<StringOrU64>()? {
                    match value {
                        StringOrU64::String(s) => {
                            if s.len() <= MAX_STRING_LEN {
                                vec_string.push(s);
                            } else {
                                return Err(serde::de::Error::custom("string too long"));
                            }
                        }
                        StringOrU64::U64(u) => vec_u64.push(u),
                    }
                }

                match (vec_u64.is_empty(), vec_string.is_empty()) {
                    (true, false) => Ok(PrincipalValue::StringList(vec_string)),
                    (false, true) => Ok(PrincipalValue::IntegerList(vec_u64)),
                    (true, true) => Ok(PrincipalValue::StringList(vec_string)),
                    _ => Err(serde::de::Error::custom("invalid principal value")),
                }
            }
        }

        deserializer.deserialize_any(PrincipalValueVisitor)
    }
}

impl<'de> serde::Deserialize<'de> for PrincipalSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct PrincipalVisitor;

        // Deserialize the principal
        impl<'de> Visitor<'de> for PrincipalVisitor {
            type Value = PrincipalSet;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a valid principal")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: de::MapAccess<'de>,
            {
                let mut principal = PrincipalSet::default();

                while let Some(key) = map.next_key::<&str>()? {
                    let key = PrincipalField::try_parse(key)
                        .or_else(|| {
                            if key == "id" {
                                // Ignored
                                Some(PrincipalField::UsedQuota)
                            } else {
                                None
                            }
                        })
                        .ok_or_else(|| {
                            serde::de::Error::custom(format!("invalid principal field: {}", key))
                        })?;

                    let value = match key {
                        PrincipalField::Name => {
                            PrincipalValue::String(map.next_value::<String>().and_then(|v| {
                                if v.len() <= MAX_STRING_LEN {
                                    Ok(v)
                                } else {
                                    Err(serde::de::Error::custom("string too long"))
                                }
                            })?)
                        }
                        PrincipalField::Description
                        | PrincipalField::Tenant
                        | PrincipalField::Picture
                        | PrincipalField::Locale => {
                            if let Some(v) = map.next_value::<Option<String>>()? {
                                if v.len() <= MAX_STRING_LEN {
                                    PrincipalValue::String(v)
                                } else {
                                    return Err(serde::de::Error::custom("string too long"));
                                }
                            } else {
                                continue;
                            }
                        }
                        PrincipalField::Type => {
                            principal.typ = Type::parse(map.next_value()?).ok_or_else(|| {
                                serde::de::Error::custom("invalid principal type")
                            })?;
                            continue;
                        }
                        PrincipalField::Quota => map.next_value::<PrincipalValue>()?,
                        PrincipalField::Secrets
                        | PrincipalField::Emails
                        | PrincipalField::MemberOf
                        | PrincipalField::Members
                        | PrincipalField::Roles
                        | PrincipalField::Lists
                        | PrincipalField::EnabledPermissions
                        | PrincipalField::DisabledPermissions
                        | PrincipalField::Urls
                        | PrincipalField::ExternalMembers => {
                            match map.next_value::<StringOrMany>()? {
                                StringOrMany::One(v) => PrincipalValue::StringList(vec![v]),
                                StringOrMany::Many(v) => {
                                    if !v.is_empty() {
                                        PrincipalValue::StringList(v)
                                    } else {
                                        continue;
                                    }
                                }
                            }
                        }
                        PrincipalField::UsedQuota => {
                            // consume and ignore
                            map.next_value::<IgnoredAny>()?;
                            continue;
                        }
                    };

                    principal.fields.insert(key, value);
                }

                Ok(principal)
            }
        }

        deserializer.deserialize_map(PrincipalVisitor)
    }
}

#[derive(Debug)]
enum StringOrU64 {
    String(String),
    U64(u64),
}

impl<'de> serde::Deserialize<'de> for StringOrU64 {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringOrU64Visitor;

        impl Visitor<'_> for StringOrU64Visitor {
            type Value = StringOrU64;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or u64")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() <= MAX_STRING_LEN {
                    Ok(StringOrU64::String(value.to_string()))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v.len() <= MAX_STRING_LEN {
                    Ok(StringOrU64::String(v))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                Ok(StringOrU64::U64(value))
            }
        }

        deserializer.deserialize_any(StringOrU64Visitor)
    }
}

#[derive(Debug)]
enum StringOrMany {
    One(String),
    Many(Vec<String>),
}

impl<'de> serde::Deserialize<'de> for StringOrMany {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct StringOrManyVisitor;

        impl<'de> Visitor<'de> for StringOrManyVisitor {
            type Value = StringOrMany;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a string or a sequence of strings")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if value.len() <= MAX_STRING_LEN {
                    Ok(StringOrMany::One(value.into()))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_string<E>(self, v: String) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                if v.len() <= MAX_STRING_LEN {
                    Ok(StringOrMany::One(v))
                } else {
                    Err(serde::de::Error::custom("string too long"))
                }
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: de::SeqAccess<'de>,
            {
                let mut vec = Vec::new();

                while let Some(value) = seq.next_element::<String>()? {
                    vec.push(value);
                }

                Ok(StringOrMany::Many(vec))
            }
        }

        deserializer.deserialize_any(StringOrManyVisitor)
    }
}

impl Permission {
    pub fn all() -> impl Iterator<Item = Permission> {
        (0..Permission::COUNT).filter_map(Permission::from_id)
    }

    pub const fn is_user_permission(&self) -> bool {
        matches!(
            self,
            Permission::Authenticate
                | Permission::AuthenticateOauth
                | Permission::EmailSend
                | Permission::EmailReceive
                | Permission::ManageEncryption
                | Permission::ManagePasswords
                | Permission::JmapEmailGet
                | Permission::JmapMailboxGet
                | Permission::JmapThreadGet
                | Permission::JmapIdentityGet
                | Permission::JmapEmailSubmissionGet
                | Permission::JmapPushSubscriptionGet
                | Permission::JmapSieveScriptGet
                | Permission::JmapVacationResponseGet
                | Permission::JmapQuotaGet
                | Permission::JmapBlobGet
                | Permission::JmapEmailSet
                | Permission::JmapMailboxSet
                | Permission::JmapIdentitySet
                | Permission::JmapEmailSubmissionSet
                | Permission::JmapPushSubscriptionSet
                | Permission::JmapSieveScriptSet
                | Permission::JmapVacationResponseSet
                | Permission::JmapEmailChanges
                | Permission::JmapMailboxChanges
                | Permission::JmapThreadChanges
                | Permission::JmapIdentityChanges
                | Permission::JmapEmailSubmissionChanges
                | Permission::JmapQuotaChanges
                | Permission::JmapEmailCopy
                | Permission::JmapBlobCopy
                | Permission::JmapEmailImport
                | Permission::JmapEmailParse
                | Permission::JmapEmailQueryChanges
                | Permission::JmapMailboxQueryChanges
                | Permission::JmapEmailSubmissionQueryChanges
                | Permission::JmapSieveScriptQueryChanges
                | Permission::JmapQuotaQueryChanges
                | Permission::JmapEmailQuery
                | Permission::JmapMailboxQuery
                | Permission::JmapEmailSubmissionQuery
                | Permission::JmapSieveScriptQuery
                | Permission::JmapQuotaQuery
                | Permission::JmapSearchSnippet
                | Permission::JmapSieveScriptValidate
                | Permission::JmapBlobLookup
                | Permission::JmapBlobUpload
                | Permission::JmapEcho
                | Permission::ImapAuthenticate
                | Permission::ImapAclGet
                | Permission::ImapAclSet
                | Permission::ImapMyRights
                | Permission::ImapListRights
                | Permission::ImapAppend
                | Permission::ImapCapability
                | Permission::ImapId
                | Permission::ImapCopy
                | Permission::ImapMove
                | Permission::ImapCreate
                | Permission::ImapDelete
                | Permission::ImapEnable
                | Permission::ImapExpunge
                | Permission::ImapFetch
                | Permission::ImapIdle
                | Permission::ImapList
                | Permission::ImapLsub
                | Permission::ImapNamespace
                | Permission::ImapRename
                | Permission::ImapSearch
                | Permission::ImapSort
                | Permission::ImapSelect
                | Permission::ImapExamine
                | Permission::ImapStatus
                | Permission::ImapStore
                | Permission::ImapSubscribe
                | Permission::ImapThread
                | Permission::Pop3Authenticate
                | Permission::Pop3List
                | Permission::Pop3Uidl
                | Permission::Pop3Stat
                | Permission::Pop3Retr
                | Permission::Pop3Dele
                | Permission::SieveAuthenticate
                | Permission::SieveListScripts
                | Permission::SieveSetActive
                | Permission::SieveGetScript
                | Permission::SievePutScript
                | Permission::SieveDeleteScript
                | Permission::SieveRenameScript
                | Permission::SieveCheckScript
                | Permission::SieveHaveSpace
                | Permission::SpamFilterClassify
                | Permission::SpamFilterTrain
                | Permission::DavSyncCollection
                | Permission::DavExpandProperty
                | Permission::DavPrincipalAcl
                | Permission::DavPrincipalMatch
                | Permission::DavPrincipalSearchPropSet
                | Permission::DavFilePropFind
                | Permission::DavFilePropPatch
                | Permission::DavFileGet
                | Permission::DavFileMkCol
                | Permission::DavFileDelete
                | Permission::DavFilePut
                | Permission::DavFileCopy
                | Permission::DavFileMove
                | Permission::DavFileLock
                | Permission::DavFileAcl
                | Permission::DavCardPropFind
                | Permission::DavCardPropPatch
                | Permission::DavCardGet
                | Permission::DavCardMkCol
                | Permission::DavCardDelete
                | Permission::DavCardPut
                | Permission::DavCardCopy
                | Permission::DavCardMove
                | Permission::DavCardLock
                | Permission::DavCardAcl
                | Permission::DavCardQuery
                | Permission::DavCardMultiGet
                | Permission::DavCalPropFind
                | Permission::DavCalPropPatch
                | Permission::DavCalGet
                | Permission::DavCalMkCol
                | Permission::DavCalDelete
                | Permission::DavCalPut
                | Permission::DavCalCopy
                | Permission::DavCalMove
                | Permission::DavCalLock
                | Permission::DavCalAcl
                | Permission::DavCalQuery
                | Permission::DavCalMultiGet
                | Permission::DavCalFreeBusyQuery
                | Permission::CalendarAlarms
                | Permission::CalendarSchedulingSend
                | Permission::CalendarSchedulingReceive
        )
    }

    #[cfg(not(feature = "enterprise"))]
    pub const fn is_tenant_admin_permission(&self) -> bool {
        false
    }

}
