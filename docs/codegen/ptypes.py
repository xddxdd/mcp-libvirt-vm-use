from . import codegen
import types

_types_by_name = {}
_types = []

def type_exists(name):
    return name in _types_by_name

def lookup_type(name):
    return _types_by_name[name]

def get_named_types():
    return _types

# Some attribute are propagated from member to the type as they really
# are part of the type definition, rather than the member. This applies
# only to attributes that affect pointer or array attributes, as these
# are member local types, unlike e.g. a Struct that may be used by
# other members
propagated_attributes=["ptr_array", "nonnull", "chunk", "zero_terminated"]

valid_attributes_generic=set([
    # embedded/appended at the end of the resulting C structure
    'end',
    # the C structure contains a pointer to data
    # for instance we want to write an array to an allocated array
    # The demarshaller allocates space for data that this pointer points to
    'to_ptr',
    # write output to this C structure
    'ctype',
    # prefix for flags/values enumerations
    'prefix',
    # used in demarshaller to use directly data from message without a copy
    'nocopy',
    # store member array in a pointer
    # Has an argument which is the name of a C field which will store the
    # array length
    # The demarshaller stores a reference to the original message buffer so
    # you should keep a reference to the original message to avoid a dangling
    # pointer
    # Is useful for large buffers to avoid extra memory allocation and copying
    'as_ptr',
    # do not generate marshall code
    # used for last members to be able to marshall them manually
    'nomarshal',
    # ??? not used by python code
    'zero_terminated',
    # force generating marshaller code, applies to pointers which by
    # default are not marshalled (submarshallers are generated)
    'marshall',
    # this pointer member cannot be null
    'nonnull',
    # this flag member contains only a single flag
    'unique_flag',
    # represent array as an array of pointers (in the C structure)
    # for instance a "Type name[size] @ptr_array" in the protocol
    # will be stored in a "Type *name[0]" field in the C structure
    'ptr_array',
    'outvar',
    # C structure has an anonymous member (used in switch)
    'anon',
    # the C structure contains a pointer to a SpiceChunks structure.
    # The SpiceChunks structure is allocated inside the demarshalled
    # buffer and data will point to original message.
    'chunk',
    # this channel is contained in an #ifdef section
    # the argument specifies the preprocessor define to check
    'ifdef',
    # write this member as zero on network
    # when marshalling, a zero field is written to the network
    # when demarshalling, the field is read from the network and discarded
    'zero',
    # this attribute does not exist on the network, fill just structure with the value
    'virtual',
    # generate C structure declarations from protocol definition
    'declare',
])

attributes_with_arguments=set([
    'ctype',
    'prefix',
    'as_ptr',
    'outvar',
    'ifdef',
    'virtual',
])

# these attributes specify output format, only one can be set
output_attributes = set([
    'end',
    'to_ptr',
    'as_ptr',
    'ptr_array',
    'zero',
    'chunk',
])

def fix_attributes(attribute_list, valid_attributes=valid_attributes_generic):
    attrs = {}
    for attr in attribute_list:
        name = attr[0][1:] # [1:] strips the leading '@' from the name
        lst = attr[1:]
        if not name in valid_attributes:
            raise Exception("Attribute %s not recognized" % name)
        if not name in attributes_with_arguments:
            if len(lst) > 0:
                raise Exception("Attribute %s specified with options" % name)
        elif len(lst) > 1:
            raise Exception("Attribute %s has more than 1 argument" % name)
        attrs[name] = lst

    # only one output attribute can be set
    if len(output_attributes.intersection(list(attrs.keys()))) > 1:
        raise Exception("Multiple output type attributes specified %s" % output_attrs)

    return attrs

class Type:
    def __init__(self):
        self.attributes = {}
        self.registered = False
        self.name = None

    def has_name(self):
        return self.name != None

    def is_primitive(self):
        return False

    def is_fixed_sizeof(self):
        return True

    def is_extra_size(self):
        return False

    def contains_extra_size(self):
        return False

    def is_fixed_nw_size(self):
        return True

    def is_array(self):
        return isinstance(self, ArrayType)

    def contains_member(self, member):
        return False

    def is_struct(self):
        return isinstance(self, StructType)

    def is_pointer(self):
        return isinstance(self, PointerType)

    def get_num_pointers(self):
        return 0

    def get_pointer_names(self, marshalled):
        return []

    def sizeof(self):
        return "sizeof(%s)" % (self.c_type())

    def __repr__(self):
        return self.__str__()

    def __str__(self):
        if self.name != None:
            return self.name
        return "anonymous type"

    def resolve(self):
        return self

    def register(self):
        if self.registered or self.name == None:
            return
        self.registered = True
        if self.name in _types_by_name:
            raise Exception("Type %s already defined" % self.name)
        _types.append(self)
        _types_by_name[self.name] = self

    def has_attr(self, name):
        return name in self.attributes

class TypeRef(Type):
    def __init__(self, name):
        Type.__init__(self)
        self.name = name

    def __str__(self):
        return "ref to %s" % (self.name)

    def resolve(self):
        if self.name not in _types_by_name:
            raise Exception("Unknown type %s" % self.name)
        return _types_by_name[self.name]

    def register(self):
        assert False, "Can't register TypeRef!"


class IntegerType(Type):
    def __init__(self, bits, signed):
        Type.__init__(self)
        self.bits = bits
        self.signed = signed

        if signed:
            self.name = "int%d" % bits
        else:
            self.name = "uint%d" % bits

    def primitive_type(self):
        return self.name

    def c_type(self):
        return self.name + "_t"

    def get_fixed_nw_size(self):
        return self.bits // 8

    def is_primitive(self):
        return True

class TypeAlias(Type):
    def __init__(self, name, the_type, attribute_list):
        Type.__init__(self)
        self.name = name
        self.the_type = the_type
        self.attributes = fix_attributes(attribute_list)

    def primitive_type(self):
        return self.the_type.primitive_type()

    def resolve(self):
        self.the_type = self.the_type.resolve()
        return self

    def __str__(self):
        return "alias %s" % self.name

    def is_primitive(self):
        return self.the_type.is_primitive()

    def is_fixed_sizeof(self):
        return self.the_type.is_fixed_sizeof()

    def is_fixed_nw_size(self):
        return self.the_type.is_fixed_nw_size()

    def get_fixed_nw_size(self):
        return self.the_type.get_fixed_nw_size()

    def get_num_pointers(self):
        return self.the_type.get_num_pointers()

    def get_pointer_names(self, marshalled):
        return self.the_type.get_pointer_names(marshalled)

    def c_type(self):
        if self.has_attr("ctype"):
            return self.attributes["ctype"][0]
        return self.the_type.c_type()

class EnumBaseType(Type):
    def is_enum(self):
        return isinstance(self, EnumType)

    def primitive_type(self):
        return "uint%d" % (self.bits)

    def c_type(self):
        return "uint%d_t" % (self.bits)

    def c_name(self):
        return codegen.prefix_camel(self.name)

    def c_enumname(self, value):
        return self.c_enumname_by_name(self.names[value])

    def c_enumname_by_name(self, name):
        if self.has_attr("prefix"):
            return self.attributes["prefix"][0] + name
        return codegen.prefix_underscore_upper(self.name.upper(), name)

    def is_primitive(self):
        return True

    def get_fixed_nw_size(self):
        return self.bits // 8

    # generates a value-name table suitable for use with the wireshark protocol
    # dissector
    def c_describe(self, writer):
        writer.write("static const value_string %s_vs[] = " % codegen.prefix_underscore_lower(self.name))
        writer.begin_block()
        values = list(self.names.keys())
        values.sort()
        for i in values:
            writer.write("{ ")
            writer.write(self.c_enumname(i))
            writer.write(", \"%s\" }," % self.names[i])
            writer.newline()
        writer.write("{ 0, NULL }")
        writer.end_block(semicolon=True)
        writer.newline()


class EnumType(EnumBaseType):
    def __init__(self, bits, name, enums, attribute_list):
        Type.__init__(self)
        self.bits = bits
        self.name = name

        last = -1
        names = {}
        values = {}
        attributes = {}
        for v in enums:
            name = v[0]
            if v[1] is not None:
                value = v[1]
            else:
                value = last + 1
            last = value

            assert value not in names
            names[value] = name
            values[name] = value
            attributes[value] = fix_attributes(v[2], set(['deprecated']))

        self.names = names
        self.values = values
        self.val_attributes = attributes

        self.attributes = fix_attributes(attribute_list)

    def __str__(self):
        return "enum %s" % self.name

    def c_define(self, writer):
        writer.write("typedef enum ")
        writer.write(self.c_name())
        writer.begin_block()
        values = list(self.names.keys())
        values.sort()
        current_default = 0
        for i in values:
            writer.write(self.c_enumname(i))
            if i != current_default:
                writer.write(" = %d" % (i))
            if 'deprecated' in self.val_attributes[i]:
                writer.write(' SPICE_GNUC_DEPRECATED')
            writer.write(",")
            writer.newline()
            current_default = i + 1
        writer.newline()
        writer.write(codegen.prefix_underscore_upper(self.name.upper(), "ENUM_END"))
        writer.newline()
        writer.end_block(newline=False)
        writer.write(" ")
        writer.write(self.c_name())
        writer.write(";")
        writer.newline()
        writer.newline()

class FlagsType(EnumBaseType):
    def __init__(self, bits, name, flags, attribute_list):
        Type.__init__(self)
        self.bits = bits
        self.name = name

        last = -1
        names = {}
        values = {}
        attributes = {}
        for v in flags:
            name = v[0]
            if v[1] is not None:
                value = v[1]
            else:
                value = last + 1
            last = value

            assert value not in names
            names[value] = name
            values[name] = value
            attributes[value] = fix_attributes(v[2], set(['deprecated']))

        self.names = names
        self.values = values
        self.val_attributes = attributes

        self.attributes = fix_attributes(attribute_list)

    def __str__(self):
        return "flags %s" % self.name

    def c_define(self, writer):
        writer.write("typedef enum ")
        writer.write(self.c_name())
        writer.begin_block()
        values = list(self.names.keys())
        values.sort()
        mask = 0
        for i in values:
            writer.write(self.c_enumname(i))
            mask = mask |  (1<<i)
            writer.write(" = (1 << %d)" % (i))
            if 'deprecated' in self.val_attributes[i]:
                writer.write(' SPICE_GNUC_DEPRECATED')
            writer.write(",")
            writer.newline()
            current_default = i + 1
        writer.newline()
        writer.write(codegen.prefix_underscore_upper(self.name.upper(), "MASK"))
        writer.write(" = 0x%x" % (mask))
        writer.newline()
        writer.end_block(newline=False)
        writer.write(" ")
        writer.write(self.c_name())
        writer.write(";")
        writer.newline()
        writer.newline()

class ArrayType(Type):
    def __init__(self, element_type, size):
        Type.__init__(self)
        self.name = None

        self.element_type = element_type
        self.size = size

    def __str__(self):
        if self.size == None:
            return "%s[]" % (str(self.element_type))
        else:
            return "%s[%s]" % (str(self.element_type), str(self.size))

    def resolve(self):
        self.element_type = self.element_type.resolve()
        return self

    def is_constant_length(self):
        return isinstance(self.size, int)

    def is_remaining_length(self):
        return isinstance(self.size, str) and len(self.size) == 0

    def is_identifier_length(self):
        return isinstance(self.size, str) and len(self.size) > 0

    def is_image_size_length(self):
        if isinstance(self.size, int) or isinstance(self.size, str):
            return False
        return self.size[0] == "image_size"

    def is_cstring_length(self):
        if isinstance(self.size, int) or isinstance(self.size, str):
            return False
        return self.size[0] == "cstring"

    def is_fixed_sizeof(self):
        return self.is_constant_length() and self.element_type.is_fixed_sizeof()

    def is_fixed_nw_size(self):
        return self.is_constant_length() and self.element_type.is_fixed_nw_size()

    def get_fixed_nw_size(self):
        if not self.is_fixed_nw_size():
            raise Exception("Not a fixed size type")

        return self.element_type.get_fixed_nw_size() * self.size

    def get_num_pointers(self):
        element_count = self.element_type.get_num_pointers()
        if element_count  == 0:
            return 0
        if self.is_constant_length():
            return element_count * self.size
        raise Exception("Pointers in dynamic arrays not supported")

    def get_pointer_names(self, marshalled):
        element_count = self.element_type.get_num_pointers()
        if element_count  == 0:
            return []
        raise Exception("Pointer names in arrays not supported")

    def is_extra_size(self):
        return self.has_attr("ptr_array")

    def contains_extra_size(self):
        return self.element_type.contains_extra_size() or self.has_attr("chunk")

    def sizeof(self):
        return "%s * %s" % (self.element_type.sizeof(), self.size)

    def c_type(self):
        return self.element_type.c_type()

    def check_valid(self, member):
        # If the size is not constant the array has to be allocated in some
        # way in the output and so there must be a specification for the
        # output (as default is write into the C structure all data).
        # The only exceptions are when the length is constant (in this case
        # a constant length array in the C structure is used) or a pointer
        # (in this case the pointer allocate the array).
        if (not self.is_constant_length()
            and len(output_attributes.intersection(list(member.attributes.keys()))) == 0
            and not member.member_type.is_pointer()):
            raise Exception("Array length must be a constant or some output specifiers must be set")
        # These attribute corresponds to specific structure size
        if member.has_attr("chunk") or member.has_attr("as_ptr"):
            return
        # These attribute indicate that the array is stored in the structure
        # as a pointer of the array. If there's no way to retrieve the length
        # of the array give error, as the user has no way to do bound checks
        if member.has_attr("to_ptr") or member.has_attr("ptr_array"):
            if not (self.is_identifier_length() or self.is_constant_length()):
                raise Exception("Unsecure, no length of array")
            return
        # This attribute indicate that the array is store at the end
        # of the structure, the user will compute the length from the
        # entire message size
        if member.has_end_attr():
            return
        # Avoid bug, the array has no length specified and no space
        # would be allocated
        if self.is_remaining_length():
            raise Exception('C output array is not allocated')
        # For constant length (like "foo[5]") the field is a sized array
        # For identifier automatically a pointer to allocated data is store,
        # in this case user can read the size using the other field specified
        # by the identifier
        if self.is_constant_length() or self.is_identifier_length():
            return
        raise NotImplementedError('unknown array %s' % str(self))

    def generate_c_declaration(self, writer, member):
        self.check_valid(member)
        name = member.name
        if member.has_attr("chunk"):
            return writer.writeln('SpiceChunks *%s;' % name)
        if member.has_attr("as_ptr"):
            len_var = member.attributes["as_ptr"][0]
            writer.writeln('uint32_t %s;' % len_var)
            return writer.writeln('%s *%s;' % (self.c_type(), name))
        if member.has_attr("to_ptr"):
            return writer.writeln('%s *%s;' % (self.c_type(), name))
        if member.has_attr("ptr_array"):
            return writer.writeln('%s *%s[0];' % (self.c_type(), name))
        if member.has_end_attr():
            return writer.writeln('%s %s[0];' % (self.c_type(), name))
        if self.is_constant_length() and self.has_attr("zero_terminated"):
            return writer.writeln('%s %s[%s];' % (self.c_type(), name, self.size + 1))
        if self.is_constant_length():
            return writer.writeln('%s %s[%s];' % (self.c_type(), name, self.size))
        if self.is_identifier_length():
            return writer.writeln('%s *%s;' % (self.c_type(), name))
        raise NotImplementedError('unknown array %s' % str(self))

class PointerType(Type):
    def __init__(self, target_type):
        Type.__init__(self)
        self.name = None
        self.target_type = target_type

    def __str__(self):
        return "%s*" % (str(self.target_type))

    def resolve(self):
        self.target_type = self.target_type.resolve()
        return self

    def is_fixed_nw_size(self):
        return True

    def is_primitive(self):
        return True

    def primitive_type(self):
        return "uint32"

    def get_fixed_nw_size(self):
        return 4

    def c_type(self):
        return "uint32_t"

    def contains_extra_size(self):
        return True

    def get_num_pointers(self):
        return 1

    def generate_c_declaration(self, writer, member):
        target_type = self.target_type
        is_array = target_type.is_array()
        if not is_array or target_type.is_identifier_length():
            writer.writeln("%s *%s;" % (target_type.c_type(), member.name))
            return
        raise NotImplementedError('Some pointers to array declarations are not implemented %s' %
member)

class Containee:
    def __init__(self):
        self.attributes = {}

    def is_switch(self):
        return False

    def is_pointer(self):
        return not self.is_switch() and self.member_type.is_pointer()

    def is_array(self):
        return not self.is_switch() and self.member_type.is_array()

    def is_struct(self):
        return not self.is_switch() and self.member_type.is_struct()

    def is_primitive(self):
        return not self.is_switch() and self.member_type.is_primitive()

    def has_attr(self, name):
        return name in self.attributes

    def has_end_attr(self):
        return self.has_attr("end")

class Member(Containee):
    def __init__(self, name, member_type, attribute_list):
        Containee.__init__(self)
        self.name = name
        self.member_type = member_type
        self.attributes = fix_attributes(attribute_list)

    def resolve(self, container):
        self.container = container
        self.member_type = self.member_type.resolve()
        self.member_type.register()
        for i in propagated_attributes:
            if self.has_attr(i):
                self.member_type.attributes[i] = self.attributes[i]
                if self.member_type.is_pointer():
                    self.member_type.target_type.attributes[i] = self.attributes[i]
        return self

    def contains_member(self, member):
        return self.member_type.contains_member(member)

    def is_primitive(self):
        return self.member_type.is_primitive()

    def is_fixed_sizeof(self):
        if self.has_end_attr():
            return False
        return self.member_type.is_fixed_sizeof()

    def is_extra_size(self):
        return self.has_end_attr() or self.has_attr("to_ptr") or self.member_type.is_extra_size()

    def is_fixed_nw_size(self):
        if self.has_attr("virtual"):
            return True
        return self.member_type.is_fixed_nw_size()

    def get_fixed_nw_size(self):
        if self.has_attr("virtual"):
            return 0
        size = self.member_type.get_fixed_nw_size()
        return size

    def contains_extra_size(self):
        return self.member_type.contains_extra_size()

    def sizeof(self):
        return self.member_type.sizeof()

    def __repr__(self):
        return "%s (%s)" % (str(self.name), str(self.member_type))

    def get_num_pointers(self):
        if self.has_attr("to_ptr"):
            return 1
        return self.member_type.get_num_pointers()

    def get_pointer_names(self, marshalled):
        if self.member_type.is_pointer():
            if self.has_attr("marshall") == marshalled:
                names = [self.name]
            else:
                names = []
        else:
            names = self.member_type.get_pointer_names(marshalled)
        if self.has_attr("outvar"):
            prefix = self.attributes["outvar"][0]
            names = [prefix + "_" + name for name in names]
        return names

    def generate_c_declaration(self, writer):
        if self.has_attr("zero"):
            return
        if self.is_pointer() or self.is_array():
            self.member_type.generate_c_declaration(writer, self)
            return
        return writer.writeln("%s %s;" % (self.member_type.c_type(), self.name))

class SwitchCase:
    def __init__(self, values, member):
        self.values = values
        self.member = member
        self.members = [member]

    def get_check(self, var_cname, var_type):
        checks = []
        for v in self.values:
            if v == None:
                return "1"
            elif var_type.is_enum():
                assert v[0] == "", "Negation of enumeration in switch is not supported"
                checks.append("%s == %s" % (var_cname, var_type.c_enumname_by_name(v[1])))
            else:
                checks.append("%s(%s & %s)" % (v[0], var_cname, var_type.c_enumname_by_name(v[1])))
        return " || ".join(checks)

    def resolve(self, container):
        self.switch = container
        self.member = self.member.resolve(self)
        return self

    def get_num_pointers(self):
        return self.member.get_num_pointers()

    def get_pointer_names(self, marshalled):
        return self.member.get_pointer_names(marshalled)

class Switch(Containee):
    def __init__(self, variable, cases, name, attribute_list):
        Containee.__init__(self)
        self.variable = variable
        self.name = name
        self.cases = cases
        self.attributes = fix_attributes(attribute_list)

    def is_switch(self):
        return True

    def lookup_case_member(self, name):
        for c in self.cases:
            if c.member.name == name:
                return c.member
        return None

    def has_switch_member(self, member):
        for c in self.cases:
            if c.member == member:
                return True
        return False

    def resolve(self, container):
        self.container = container
        self.cases = [c.resolve(self) for c in self.cases]
        return self

    def __repr__(self):
        return "switch on %s %s" % (str(self.variable),str(self.name))

    def is_fixed_sizeof(self):
        # Kinda weird, but we're unlikely to have a real struct if there is an @end
        if self.has_end_attr():
            return False
        return True

    def is_fixed_nw_size(self):
        size = None
        has_default = False
        for c in self.cases:
            for v in c.values:
                if v == None:
                    has_default = True
            if not c.member.is_fixed_nw_size():
                return False
            if size == None:
                size = c.member.get_fixed_nw_size()
            elif size != c.member.get_fixed_nw_size():
                return False
        # Fixed size if all elements listed, or has default
        if has_default:
            return True
        key = self.container.lookup_member(self.variable)
        return len(self.cases) == len(key.member_type.values)

    def is_extra_size(self):
        return self.has_end_attr()

    def contains_extra_size(self):
        for c in self.cases:
            if c.member.is_extra_size():
                return True
            if c.member.contains_extra_size():
                return True
        return False

    def get_fixed_nw_size(self):
        if not self.is_fixed_nw_size():
            raise Exception("Not a fixed size type")
        size = 0
        for c in self.cases:
            size = max(size, c.member.get_fixed_nw_size())
        return size

    def sizeof(self):
        return "sizeof(((%s *)NULL)->%s)" % (self.container.c_type(),
                                             self.name)

    def contains_member(self, member):
        return False # TODO: Don't support switch deep member lookup yet

    def get_num_pointers(self):
        count = 0
        for c in self.cases:
            count = max(count, c.get_num_pointers())
        return count

    def get_pointer_names(self, marshalled):
        names = []
        for c in self.cases:
            names = names + c.get_pointer_names(marshalled)
        return names

    def generate_c_declaration(self, writer):
        if self.has_attr("anon") and len(self.cases) == 1:
            self.cases[0].member.generate_c_declaration(writer)
            return
        writer.writeln('union {')
        writer.indent()
        for m in self.cases:
            m.member.generate_c_declaration(writer)
        writer.unindent()
        writer.writeln('} %s;' % self.name)

class ContainerType(Type):
    def is_fixed_sizeof(self):
        for m in self.members:
            if not m.is_fixed_sizeof():
                return False
        return True

    def contains_extra_size(self):
        for m in self.members:
            if m.is_extra_size():
                return True
            if m.contains_extra_size():
                return True
        return False

    def is_fixed_nw_size(self):
        for i in self.members:
            if not i.is_fixed_nw_size():
                return False
        return True

    def get_fixed_nw_size(self):
        size = 0
        for i in self.members:
            size = size + i.get_fixed_nw_size()
        return size

    def contains_member(self, member):
        for m in self.members:
            if m == member or m.contains_member(member):
                return True
        return False

    def get_fixed_nw_offset(self, member):
        size = 0
        for i in self.members:
            if i == member:
                break
            if i.contains_member(member):
                size = size  + i.member_type.get_fixed_nw_offset(member)
                break
            if i.is_fixed_nw_size():
                size = size + i.get_fixed_nw_size()
        return size

    def resolve(self):
        self.members = [m.resolve(self) for m in self.members]
        return self

    def get_num_pointers(self):
        count = 0
        for m in self.members:
            count = count + m.get_num_pointers()
        return count

    def get_pointer_names(self, marshalled):
        names = []
        for m in self.members:
            names = names + m.get_pointer_names(marshalled)
        return names

    def get_nw_offset(self, member, prefix = "", postfix = ""):
        fixed = self.get_fixed_nw_offset(member)
        v = []
        container = self
        while container != None:
            members = container.members
            container = None
            for m in members:
                if m == member:
                    break
                if m.contains_member(member):
                    container = m.member_type
                    break
                if m.is_switch() and m.has_switch_member(member):
                    break
                if not m.is_fixed_nw_size():
                    v.append(prefix + m.name + postfix)
        if len(v) > 0:
            return str(fixed) + " + " + (" + ".join(v))
        else:
            return str(fixed)

    def lookup_member(self, name):
        dot = name.find('.')
        rest = None
        if dot >= 0:
            rest = name[dot+1:]
            name = name[:dot]

        member = None
        if name in self.members_by_name:
            member = self.members_by_name[name]
        else:
            for m in self.members:
                if m.is_switch():
                    member = m.lookup_case_member(name)
                    if member != None:
                        break

        if member == None:
            raise Exception("No member called %s found" % name)

        if rest != None:
            return member.member_type.lookup_member(rest)

        return member

    def generate_c_declaration(self, writer):
        if not self.has_attr('declare'):
            return
        name = self.c_type()
        writer.writeln('typedef struct %s {' % name)
        writer.indent()
        for m in self.members:
            m.generate_c_declaration(writer)
        if len(self.members) == 0:
            # make sure generated structure are not empty
            writer.writeln("char dummy[0];")
        writer.unindent()
        writer.writeln('} %s;' % name).newline()

class StructType(ContainerType):
    def __init__(self, name, members, attribute_list):
        Type.__init__(self)
        self.name = name
        self.members = members
        self.members_by_name = {}
        for m in members:
            self.members_by_name[m.name] = m
        self.attributes = fix_attributes(attribute_list)

    def __str__(self):
        if self.name == None:
            return "anonymous struct"
        else:
            return "struct %s" % self.name

    def c_type(self):
        if self.has_attr("ctype"):
            return self.attributes["ctype"][0]
        return codegen.prefix_camel(self.name)

class MessageType(ContainerType):
    def __init__(self, name, members, attribute_list):
        Type.__init__(self)
        self.name = name
        self.members = members
        self.members_by_name = {}
        for m in members:
            self.members_by_name[m.name] = m
        self.reverse_members = {} # ChannelMembers referencing this message
        self.attributes = fix_attributes(attribute_list)

    def __str__(self):
        if self.name == None:
            return "anonymous message"
        else:
            return "message %s" % self.name

    def c_name(self):
        if self.name == None:
            cms = list(self.reverse_members.keys())
            if len(cms) != 1:
                raise Exception("Unknown typename for message")
            cm = cms[0]
            channelname = cm.channel.member_name
            if channelname == None:
                channelname = ""
            else:
                channelname = channelname + "_"
            if cm.is_server:
                return "msg_" + channelname +  cm.name
            else:
                return "msgc_" + channelname +  cm.name
        else:
            return codegen.prefix_camel("Msg", self.name)

    def c_type(self):
        if self.has_attr("ctype"):
            return self.attributes["ctype"][0]
        if self.name == None:
            cms = list(self.reverse_members.keys())
            if len(cms) != 1:
                raise Exception("Unknown typename for message")
            cm = cms[0]
            channelname = cm.channel.member_name
            if channelname == None:
                channelname = ""
            if cm.is_server:
                return codegen.prefix_camel("Msg", channelname, cm.name)
            return codegen.prefix_camel("Msgc", channelname, cm.name)
        else:
            return codegen.prefix_camel("Msg", self.name)

class ChannelMember(Containee):
    def __init__(self, name, message_type, value):
        Containee.__init__(self)
        self.name = name
        self.message_type = message_type
        self.value = value

    def resolve(self, channel):
        self.channel = channel
        self.message_type = self.message_type.resolve()
        self.message_type.reverse_members[self] = 1

        return self

    def __repr__(self):
        return "%s (%s)" % (str(self.name), str(self.message_type))

class ChannelType(Type):
    def __init__(self, name, base, members, attribute_list):
        Type.__init__(self)
        self.name = name
        self.base = base
        self.member_name = None
        self.members = members
        self.attributes = fix_attributes(attribute_list)

    def __str__(self):
        if self.name == None:
            return "anonymous channel"
        else:
            return "channel %s" % self.name

    def is_fixed_nw_size(self):
        return False

    def get_client_message(self, name):
        return self.client_messages_byname[name]

    def get_server_message(self, name):
        return self.server_messages_byname[name]

    def resolve(self):
        class MessagesInfo:
            def __init__(self, is_server, messages=[], messages_byname={}):
                self.is_server = is_server
                self.messages = messages[:]
                self.messages_byname = messages_byname.copy()
                self.count = 1

                self.messages_byvalue = {}
                for m in self.messages:
                    self.messages_byvalue[m.value] = m

        if self.base is None:
            server_info = MessagesInfo(True)
            client_info = MessagesInfo(False)
        else:
            self.base = self.base.resolve()

            server_info = MessagesInfo(True, self.base.server_messages,
                                       self.base.server_messages_byname)
            client_info = MessagesInfo(False, self.base.client_messages,
                                       self.base.client_messages_byname)

            # Set default member_name, FooChannel -> foo
            self.member_name = self.name[:-7].lower()

        info = server_info
        for m in self.members:
            if m == "server":
                info = server_info
            elif m == "client":
                info = client_info
            else:
                m.is_server = info.is_server
                m = m.resolve(self)
                if not m.value:
                    m.value = info.count
                info.count = m.value + 1
                info.messages.append(m)
                if m.name in info.messages_byname:
                    raise Exception("Duplicated message name '%s' in channel '%s'" % (m.name, self.name))
                info.messages_byname[m.name] = m
                if m.value in info.messages_byvalue:
                    raise Exception("Duplicated message value %d between '%s' and '%s' in channel '%s'" % (
                        m.value, info.messages_byvalue[m.value].name, m.name, self.name))
                info.messages_byvalue[m.value] = m

        self.server_messages = server_info.messages
        self.server_messages_byname = server_info.messages_byname
        self.client_messages = client_info.messages
        self.client_messages_byname = client_info.messages_byname

        return self

class ProtocolMember:
    def __init__(self, name, channel_type, value):
        self.name = name
        self.channel_type = channel_type
        self.value = value

    def resolve(self, protocol):
        self.channel_type = self.channel_type.resolve()
        self.channel_type.member_name = self.name
        return self

    def __repr__(self):
        return "%s (%s)" % (str(self.name), str(self.channel_type))

class ProtocolType(Type):
    def __init__(self, name, channels):
        Type.__init__(self)
        self.name = name
        self.channels = channels

    def __str__(self):
        if self.name == None:
            return "anonymous protocol"
        else:
            return "protocol %s" % self.name

    def is_fixed_nw_size(self):
        return False

    def resolve(self):
        count = 1
        for m in self.channels:
            m = m.resolve(self)
            if m.value:
                count = m.value + 1
            else:
                m.value = count
                count = count + 1

        return self

class FdType(IntegerType):
    def __init__(self):
        IntegerType.__init__(self, 0, False)
        self.name = "fd"

    def c_type(self):
        return "int"

int8 = IntegerType(8, True)
uint8 = IntegerType(8, False)
int16 = IntegerType(16, True)
uint16 = IntegerType(16, False)
int32 = IntegerType(32, True)
uint32 = IntegerType(32, False)
int64 = IntegerType(64, True)
uint64 = IntegerType(64, False)
unix_fd = FdType()
