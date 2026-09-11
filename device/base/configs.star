def base_system(product_name, board, platform_config, bundles, component_config_values = []):
    return {
        "product_name": product_name,
        "board": board,
        "platform_config": platform_config,
        "bundles": bundles,
        "component_config_values": component_config_values,
    }

def graphical_system(product_name, board, platform_config, bundles, compositor, display_driver, component_config_values = []):
    if not compositor:
        fail("graphical products require a compositor package")
    if not display_driver:
        fail("graphical products require a display driver package")
    return base_system(
        product_name = product_name,
        board = board,
        platform_config = platform_config,
        bundles = bundles,
        component_config_values = component_config_values,
    )
