use std::borrow::Cow;

use source_map::SpanWithSource;

use crate::{
	types::{
		properties::{PropertyKey, PropertyValue, Publicity},
		TypeStore,
	},
	Constant, Environment, Type, TypeId,
};

use super::{
	constant_functions::{ConstantFunctionError, ConstantOutput},
	objects::ObjectBuilder,
};

use std::{collections::HashMap, fmt};

use regress::{backends, Flags, Regex};

use crate::BinarySerializable;

#[derive(Debug, Clone)]
pub struct RegExp {
	source: String,
	re: Regex,
	groups: u32,
	named_group_indices: HashMap<String, u16>,
	flags_unsupported: bool,
	used: bool,
}

impl RegExp {
	pub fn new(pattern: &str, flag_options: Option<&str>) -> Result<Self, String> {
		let source = if let Some(flag_options) = flag_options {
			format!("/{pattern}/{flag_options}")
		} else {
			format!("/{pattern}/")
		};

		let mut flags = Flags::default();
		let mut flags_unsupported = false;

		if let Some(flag_options) = flag_options {
			for flag in flag_options.chars() {
				match flag {
					'd' => flags_unsupported = true, // indices for substring matches are not supported
					'g' => flags_unsupported = true, // stateful regex is not supported
					'i' => flags.icase = true,
					'm' => flags.multiline = true,
					's' => flags.dot_all = true,
					'u' => flags.unicode = true,
					'v' => flags.unicode_sets = true,
					'y' => flags_unsupported = true, // sticky search is not supported
					_ => panic!("Unknown flag: {flag:?}"),
				}
			}
		}

		let compiled_regex = {
			let mut ire = backends::try_parse(pattern.chars().map(u32::from), flags)
				.map_err(|err| err.text)?;
			if !flags.no_opt {
				backends::optimize(&mut ire);
			}

			backends::emit(&ire)
		};

		// dbg!(&compiled_regex);

		// let insns = compiled_regex.insns;
		// let brackets = compiled_regex.brackets;
		// let start_pred = compiled_regex.start_pred;
		// let loops = compiled_regex.loops;
		let groups = compiled_regex.groups + 1;
		let named_group_indices = compiled_regex.named_group_indices.clone();
		// let flags = compiled_regex.flags;

		let re = Regex::from(compiled_regex);

		Ok(Self { source, re, groups, named_group_indices, flags_unsupported, used: false })
	}

	pub fn source(&self) -> &str {
		&self.source
	}

	pub fn used(&self) -> bool {
		self.used
	}

	pub(crate) fn exec(
		&self,
		pattern_type_id: TypeId,
		types: &mut TypeStore,
		environment: &mut Environment,
		call_site: SpanWithSource,
	) -> TypeId {
		let pattern_type = types.get_type_by_id(pattern_type_id);

		match (self.flags_unsupported, pattern_type) {
			(false, Type::Constant(Constant::String(pattern))) => {
				self.exec_constant(pattern.clone(), pattern_type_id, types, environment, call_site)
			}
			_ => self.exec_variable(types, environment, call_site),
		}
	}

	pub(crate) fn exec_constant(
		&self,
		pattern: String,
		pattern_type_id: TypeId,
		types: &mut TypeStore,
		environment: &mut Environment,
		call_site: SpanWithSource,
	) -> TypeId {
		let mut object =
			ObjectBuilder::new(Some(TypeId::ARRAY_TYPE), types, call_site, &mut environment.info);

		object.append(
			environment,
			Publicity::Public,
			PropertyKey::String(Cow::Borrowed("input")),
			PropertyValue::Value(pattern_type_id),
			call_site,
		);

		match self.re.find(&pattern) {
			Some(match_) => {
				{
					let index = types.new_constant_type(Constant::Number(
						(match_.start() as f64).try_into().unwrap(),
					));
					object.append(
						environment,
						Publicity::Public,
						PropertyKey::String(Cow::Borrowed("index")),
						PropertyValue::Value(index),
						call_site,
					);
				}

				for (idx, group) in match_.groups().enumerate() {
					let key = PropertyKey::from_usize(idx);
					let value = match group {
						Some(range) => {
							types.new_constant_type(Constant::String(pattern[range].to_string()))
						}
						None => todo!(),
					};

					object.append(
						environment,
						Publicity::Public,
						key,
						PropertyValue::Value(value),
						call_site,
					);
				}

				{
					let named_groups = {
						let mut named_groups_object = ObjectBuilder::new(
							Some(TypeId::NULL_TYPE),
							types,
							call_site,
							&mut environment.info,
						);

						for (name, group) in match_.named_groups() {
							let key = PropertyKey::String(Cow::Owned(name.to_string()));
							let value = match group {
								Some(range) => types.new_constant_type(Constant::String(
									pattern[range].to_string(),
								)),
								None => todo!(),
							};

							named_groups_object.append(
								environment,
								Publicity::Public,
								key,
								PropertyValue::Value(value),
								call_site,
							);
						}

						named_groups_object.build_object()
					};

					object.append(
						environment,
						Publicity::Public,
						PropertyKey::String(Cow::Borrowed("groups")),
						PropertyValue::Value(named_groups),
						call_site,
					);
				}

				{
					let length = types.new_constant_type(Constant::Number(
						(self.groups as f64).try_into().unwrap(),
					));

					object.append(
						environment,
						Publicity::Public,
						PropertyKey::String("length".into()),
						PropertyValue::Value(length),
						call_site,
					);
				}

				object.build_object()
			}
			None => TypeId::NULL_TYPE,
		}
	}

	pub(crate) fn exec_variable(
		&self,
		types: &mut TypeStore,
		environment: &mut Environment,
		call_site: SpanWithSource,
	) -> TypeId {
		let mut object =
			ObjectBuilder::new(Some(TypeId::ARRAY_TYPE), types, call_site, &mut environment.info);

		object.append(
			environment,
			Publicity::Public,
			PropertyKey::String(Cow::Borrowed("input")),
			PropertyValue::Value(TypeId::STRING_TYPE),
			call_site,
		);

		{
			object.append(
				environment,
				Publicity::Public,
				PropertyKey::String(Cow::Borrowed("index")),
				PropertyValue::Value(TypeId::NUMBER_TYPE),
				call_site,
			);
		}

		for idx in 0..self.groups {
			let key = PropertyKey::from_usize(idx as usize);

			object.append(
				environment,
				Publicity::Public,
				key,
				PropertyValue::Value(TypeId::STRING_TYPE),
				call_site,
			);
		}

		{
			let named_groups = {
				let mut named_groups_object = ObjectBuilder::new(
					Some(TypeId::NULL_TYPE),
					types,
					call_site,
					&mut environment.info,
				);

				for (name, _i) in self.named_group_indices.iter() {
					let key = PropertyKey::String(Cow::Owned(name.to_string()));

					named_groups_object.append(
						environment,
						Publicity::Public,
						key,
						PropertyValue::Value(TypeId::STRING_TYPE),
						call_site,
					);
				}

				named_groups_object.build_object()
			};

			object.append(
				environment,
				Publicity::Public,
				PropertyKey::String(Cow::Borrowed("groups")),
				PropertyValue::Value(named_groups),
				call_site,
			);
		}

		{
			let length =
				types.new_constant_type(Constant::Number((self.groups as f64).try_into().unwrap()));

			object.append(
				environment,
				Publicity::Public,
				PropertyKey::String("length".into()),
				PropertyValue::Value(length),
				call_site,
			);
		}

		types.new_or_type(object.build_object(), TypeId::NULL_TYPE)
	}
}

impl fmt::Display for RegExp {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.source)
	}
}

// TODO: Optimize
impl BinarySerializable for RegExp {
	fn serialize(self, buf: &mut Vec<u8>) {
		self.source.serialize(buf);
	}

	fn deserialize<I: Iterator<Item = u8>>(iter: &mut I, source_id: source_map::SourceId) -> Self {
		let source = String::deserialize(iter, source_id);

		let (pattern, flags) = source[1..].rsplit_once('/').unwrap();
		let flags = if flags.is_empty() { None } else { Some(flags) };

		Self::new(pattern, flags).unwrap()
	}
}